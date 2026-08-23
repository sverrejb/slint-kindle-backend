//! Slint platform backend for Kindles.
//!
//! # Usage
//!
//! ```no_run
//! slint::include_modules!();
//!
//! static DEFAULT_FONT: &[u8] = include_bytes!("../ui/MyFont.ttf");
//! static SERIF_FONT: &[u8] = include_bytes!("../ui/MySerif.ttf");
//!
//! fn main() {
//!     let backend = slint_backend_kindle::install(DEFAULT_FONT)
//!         .expect("failed to install Kindle backend");
//!     let app = AppWindow::new().expect("failed to create window");
//!     backend.register_font_from_memory(SERIF_FONT).expect("failed to register font");
//!     app.run().expect("event loop error");
//! }
//! ```

mod framebuffer;
mod platform;
mod power;
mod touch;
mod wakeup;

use platform::KindlePlatform;
use slint::platform::WindowAdapter;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) type OnWakeCallback = Rc<RefCell<Option<Box<dyn FnMut()>>>>;

/// How often to wake from suspend-to-RAM and how long to stay awake afterwards.
///
/// Pass to [`KindleBackend::set_wake_schedule`] to opt in. Without it, the
/// backend never suspends the SoC — the event loop just blocks in `poll(2)`,
/// which is fine for plugged-in use but burns battery.
///
/// Touch activity during the awake window resets `stay_awake`, exactly like
/// the device's normal idle timer.
#[derive(Debug, Clone, Copy)]
pub struct WakeSchedule {
    /// Time between scheduled wakes from suspend.
    pub wake_interval: Duration,
    /// How long to stay awake after a wake or the last touch.
    pub stay_awake: Duration,
}

/// Typestate markers
pub struct NoSchedule;
pub struct Scheduled;

/// Returned by [`install`]. Use it to add more fonts and configure power.
///
/// A new backend is [`NoSchedule`]. Calling
/// [`set_wake_schedule`](KindleBackend::set_wake_schedule) turns it into a
/// [`Scheduled`] one, and only that form has
/// [`on_wake`](KindleBackend::on_wake) — so you can't set a wake callback
/// without first setting up a wake schedule.
pub struct KindleBackend<State = NoSchedule> {
    window: Rc<MinimalSoftwareWindow>,
    wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
    on_wake: OnWakeCallback,
    black_and_white: Arc<AtomicBool>,
    _state: PhantomData<State>,
}

impl<State> KindleBackend<State> {
    /// Add an extra font (TTF/OTF) from bytes.
    ///
    /// Call this **after** you've created your window (e.g. `AppWindow::new()`).
    /// Fonts can't be added before then because Slint hasn't set up its font
    /// system yet.
    pub fn register_font_from_memory(
        &self,
        data: &'static [u8],
    ) -> Result<(), slint::PlatformError> {
        self.window
            .renderer()
            .register_font_from_memory(data)
            .map_err(|e| slint::PlatformError::Other(format!("{e}")))
    }

    /// Render in **pure black and white** (bilevel) mode: force every pixel to
    /// pure black or white, with no grey levels at all. Useful on devices where
    /// greyscale rendering causes a flicker through black to be displayed.
    ///
    /// Off by default. A change takes effect on the next render, so set it
    /// before your first window draw, toggling it later only affects pixels
    /// redrawn after that.
    pub fn set_black_and_white(&self, enabled: bool) {
        self.black_and_white.store(enabled, Ordering::Relaxed);
    }

    /// Switch state, keeping the same internals.
    fn into_state<Next>(self) -> KindleBackend<Next> {
        KindleBackend {
            window: self.window,
            wake_schedule: self.wake_schedule,
            on_wake: self.on_wake,
            black_and_white: self.black_and_white,
            _state: PhantomData,
        }
    }
}

impl KindleBackend<NoSchedule> {
    /// Turn on the wake-from-suspend cycle.
    ///
    /// The device sleeps once your app has been idle for `stay_awake`, then
    /// wakes every `wake_interval` (or earlier, e.g. on a button press) so it
    /// can refresh.
    ///
    /// Returns a [`Scheduled`] backend that lets you set
    /// [`on_wake`](KindleBackend::on_wake).
    pub fn set_wake_schedule(self, schedule: WakeSchedule) -> KindleBackend<Scheduled> {
        *self.wake_schedule.lock().expect("wake schedule poisoned") = Some(schedule);
        self.into_state()
    }
}

impl KindleBackend<Scheduled> {
    /// Change the wake schedule.
    ///
    /// The new schedule takes effect the next time the device is awake (it
    /// can't be reached while asleep).
    pub fn set_wake_schedule(&self, schedule: WakeSchedule) {
        *self.wake_schedule.lock().expect("wake schedule poisoned") = Some(schedule);
    }

    /// Turn the wake cycle back off.
    ///
    /// Forgets the [`on_wake`](KindleBackend::on_wake) callback, since it can't
    /// fire anymore. Takes effect the next time the device is awake.
    pub fn clear_wake_schedule(self) -> KindleBackend<NoSchedule> {
        *self.wake_schedule.lock().expect("wake schedule poisoned") = None;
        *self.on_wake.borrow_mut() = None;
        self.into_state()
    }

    /// Run `callback` once each time the device wakes from a scheduled suspend.
    ///
    /// Fires on the event-loop (UI) thread, after resume but before the next
    /// render. The right place to refresh state that should be current when
    /// the screen redraws after waking up, like polling an HTTP API, reading a sensor, etc.
    /// Don't rely on a `slint::Timer` to align with `wake_interval`; Slint timers
    /// run on their own schedule and may fire before or after the wake.
    ///
    /// Replaces any previously-set callback. Not invoked on the initial start of the app.
    pub fn on_wake<F: FnMut() + 'static>(&self, callback: F) {
        *self.on_wake.borrow_mut() = Some(Box::new(callback));
    }
}

/// Set up the Kindle backend and use `font_data` as the default font.
///
/// You **must** pass a font. The Kindle doesn't ship any usable system fonts,
/// so without one Slint will crash the first time it tries to draw text.
/// We write the font to a temp file and point Slint at it through an
/// environment variable so it gets used everywhere a font is needed.
///
/// Call this once at startup, before creating any windows. Use the returned
/// [`KindleBackend`] to add more fonts later.
///
/// # Errors
///
/// Fails if the temp file can't be written, or if Slint already has a
/// platform set up.
pub fn install(font_data: &[u8]) -> Result<KindleBackend, slint::PlatformError> {
    install_with_scale(font_data, 1.0)
}

/// Like [`install`] but sets a DPI scale factor. The renderer draws at
/// `physical_size / scale` logical pixels, then upscales to the framebuffer.
/// For a 300 DPI Kindle Oasis (1264x1680), use 3.0 → logical 421x560.
pub fn install_with_scale(
    font_data: &[u8],
    scale_factor: f32,
) -> Result<KindleBackend, slint::PlatformError> {
    let path = std::env::temp_dir().join("slint-kindle-default.ttf");
    std::fs::write(&path, font_data)
        .map_err(|e| slint::PlatformError::Other(format!("failed to stage default font: {e}")))?;

    // SAFETY: install() runs once at startup before any threads exist, so nothing else can read this env var at the same time.
    unsafe {
        std::env::set_var("SLINT_DEFAULT_FONT", &path);
    }

    let wake_schedule = Arc::new(Mutex::new(None));
    let on_wake: OnWakeCallback = Rc::new(RefCell::new(None));
    let black_and_white = Arc::new(AtomicBool::new(false));
    let platform = KindlePlatform::new(
        wake_schedule.clone(),
        on_wake.clone(),
        black_and_white.clone(),
        scale_factor,
    )
    .map_err(|e| slint::PlatformError::Other(format!("failed to init Kindle platform: {e}")))?;
    let window = platform.window.clone();
    slint::platform::set_platform(Box::new(platform))
        .map_err(|e| slint::PlatformError::Other(format!("{e}")))?;
    Ok(KindleBackend {
        window,
        wake_schedule,
        on_wake,
        black_and_white,
        _state: PhantomData,
    })
}

// ---------------------------------------------------------------------------
// Screen rotation
// ---------------------------------------------------------------------------

/// Screen rotation in degrees (0, 90, 180, 270).
///
/// Set by the app via [`set_rotation`]. Read by the framebuffer renderer
/// and touch input handler to transform coordinates.
/// 0° and 180° keep the same dimensions; 90° and 270° swap width/height.
static ROTATION: AtomicU32 = AtomicU32::new(0);

/// Fixed render offset determined at launch.
///
/// On the Kindle Oasis, the framebuffer's hardware rotation state is set by
/// the Amazon framework before the app takes over. If the device was at 180°
/// when the app launched, the framebuffer is already rotated 180° by hardware,
/// so every render must add 180° to compensate. If launched at 0°, no offset
/// is needed. This offset is fixed for the entire session — it never changes
/// at runtime.
static RENDER_OFFSET: AtomicU32 = AtomicU32::new(0);

/// Set the fixed render offset for this session.
///
/// Call once at startup with the initial device rotation (0 or 180).
/// On the Oasis, pass 180 if the device was held upside-down at launch,
/// otherwise pass 0.
pub fn set_render_offset(initial_rotation: u32) {
    let offset = if initial_rotation == 180 { 180 } else { 0 };
    RENDER_OFFSET.store(offset, Ordering::Relaxed);
    log::info!("[kindle] render offset set to {offset}° (initial_rotation={initial_rotation})");
}

/// Get the fixed render offset for this session.
pub fn get_render_offset() -> u32 {
    RENDER_OFFSET.load(Ordering::Relaxed)
}

/// Set the screen rotation (0, 90, 180, or 270 degrees).
///
/// Call before the event loop starts, or from the UI thread to rotate at
/// runtime. Triggers a full refresh on the next render cycle.
pub fn set_rotation(degrees: u32) {
    let normalized = degrees % 360;
    ROTATION.store(normalized, Ordering::Relaxed);
    REQUEST_FULL_REFRESH.store(true, Ordering::Relaxed);
    log::info!("[kindle] rotation set to {normalized}°");
}

/// Get the current screen rotation in degrees (0, 90, 180, or 270).
pub fn get_rotation() -> u32 {
    ROTATION.load(Ordering::Relaxed)
}

/// Enable or disable dark mode (framebuffer inversion).
/// When enabled, all pixels written to the framebuffer are inverted.
pub fn set_invert(enabled: bool) {
    framebuffer::set_invert(enabled);
    REQUEST_FULL_REFRESH.store(true, Ordering::Relaxed);
    log::info!("[kindle] invert (dark mode) set to {enabled}");
}

/// Check if dark mode (framebuffer inversion) is currently enabled.
pub fn is_inverted() -> bool {
    framebuffer::is_inverted()
}

// ---------------------------------------------------------------------------
// Sleep screen, suspend, and refresh control
// ---------------------------------------------------------------------------

/// Shared state for controlling screen refresh behavior.
pub(crate) static REQUEST_FULL_REFRESH: AtomicBool = AtomicBool::new(false);

/// Set when the device wakes from suspend. The event loop checks this
/// to force a full re-render (the framebuffer still has the sleep image).
pub(crate) static WOKE_FROM_SUSPEND: AtomicBool = AtomicBool::new(false);

/// Pre-rendered sleep screen stored as raw framebuffer bytes.
static SLEEP_SCREEN_CACHE: Mutex<Option<SleepScreenCache>> = Mutex::new(None);

/// Sleep screen background: true = white (default), false = black
static SLEEP_BG_WHITE: AtomicBool = AtomicBool::new(true);

/// Selected sleep image filename (empty = default).
static SLEEP_IMAGE_NAME: Mutex<String> = Mutex::new(String::new());

/// Global last-activity timestamp (unix seconds). Updated by the touch
/// handler on every touch event. The app's idle sleep timer reads this.
pub static LAST_ACTIVITY: AtomicI64 = AtomicI64::new(0);

/// Reset the last-activity timestamp to now. Called by the touch handler.
pub(crate) fn touch_activity() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    LAST_ACTIVITY.store(now, Ordering::Relaxed);
}

struct SleepScreenCache {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

/// Cached screen dimensions (set once from the UI thread).
static SCREEN_W: AtomicU32 = AtomicU32::new(0);
static SCREEN_H: AtomicU32 = AtomicU32::new(0);

/// Set the sleep screen background color (true = white, false = black).
pub fn set_sleep_background_white(white: bool) {
    SLEEP_BG_WHITE.store(white, Ordering::Relaxed);
}

/// Set the specific sleep image filename to use ("default" = code-generated).
/// Clears the cache so a stale image from a previous selection isn't used.
pub fn set_sleep_image_name(name: &str) {
    *SLEEP_IMAGE_NAME.lock().unwrap() = name.to_string();
    *SLEEP_SCREEN_CACHE.lock().unwrap() = None;
    log::info!("[kindle] sleep image name set to '{name}', cache cleared");
}

/// Cache the screen dimensions. Call once at startup from the UI thread.
pub fn set_screen_dimensions(w: u32, h: u32) {
    SCREEN_W.store(w, Ordering::Relaxed);
    SCREEN_H.store(h, Ordering::Relaxed);
}

/// Request a full-screen refresh on the next render.
pub fn request_full_refresh() {
    REQUEST_FULL_REFRESH.store(true, Ordering::Relaxed);
}

/// Suspend the device to RAM. Blocks until the device wakes.
pub fn suspend() {
    log::info!("[suwayomi] suspending to RAM...");
    if let Err(e) = crate::power::suspend_to_mem() {
        log::error!("[suwayomi] suspend failed: {e}");
    }
    log::info!("[suwayomi] resumed from suspend");
}

/// Force the app to re-render and flash the screen to clear the sleep image.
pub fn wake_refresh() {
    request_full_refresh();
    WOKE_FROM_SUSPEND.store(true, Ordering::Relaxed);
}

/// Draw a sleep screen image to the framebuffer and refresh.
pub fn show_sleep_screen(sleep_dir: &str) {
    use std::path::Path;

    let start = std::time::Instant::now();

    let mut fb = match framebuffer::Framebuffer::open() {
        Ok(fb) => fb,
        Err(e) => {
            log::error!("[suwayomi] sleep: failed to open framebuffer: {e}");
            return;
        }
    };

    set_screen_dimensions(fb.width, fb.height);

    let selected_name = SLEEP_IMAGE_NAME.lock().unwrap().clone();
    let bg_white = SLEEP_BG_WHITE.load(Ordering::Relaxed);
    log::info!("[suwayomi] sleep: selected='{selected_name}', bg_white={bg_white}, dir={sleep_dir}");

    // Fast path: pre-rendered cache
    {
        let cache = SLEEP_SCREEN_CACHE.lock().unwrap();
        if let Some(ref cached) = *cache {
            if cached.width == fb.width && cached.height == fb.height {
                // Double flash: flash to inverse background first to clear
                // heavy ghosting, then write the sleep image and flash again.
                let bg = if SLEEP_BG_WHITE.load(Ordering::Relaxed) { 255u8 } else { 0u8 };
                let inverse_bg = if bg == 255 { 0u8 } else { 255u8 };
                let blank = vec![inverse_bg; fb.width as usize * fb.height as usize];
                for y in 0..fb.height as usize {
                    fb.write_line(y, 0..fb.width as usize, &blank[y * fb.width as usize..(y + 1) * fb.width as usize]);
                }
                fb.refresh_full();
                fb.wait_for_update_complete();

                // Now write the cached sleep image
                let img_width = cached.width as usize;
                for y in 0..fb.height as usize {
                    let row_pixels = &cached.pixels[y * img_width..(y + 1) * img_width];
                    fb.write_line(y, 0..fb.width as usize, row_pixels);
                }
                fb.refresh_full();
                fb.wait_for_update_complete();
                log::info!("[suwayomi] sleep: displayed cached image in {:.0}ms", start.elapsed().as_millis());
                return;
            } else {
                log::warn!("[suwayomi] sleep: cache size {}x{} != fb {}x{}, regenerating", cached.width, cached.height, fb.width, fb.height);
            }
        } else {
            log::info!("[suwayomi] sleep: no cache, loading from disk");
        }
    }

    // Slow path: decode from disk or generate default
    let dir = Path::new(sleep_dir);
    let image_path = if dir.is_dir() {
        let selected = SLEEP_IMAGE_NAME.lock().unwrap().clone();
        if !selected.is_empty() && selected != "default" {
            let path = dir.join(&selected);
            if path.exists() {
                log::info!("[suwayomi] sleep: loading custom image: {}", path.display());
                Some(path)
            } else {
                log::warn!("[suwayomi] sleep: custom image '{selected}' not found in {sleep_dir}, using default");
                None
            }
        } else {
            log::info!("[suwayomi] sleep: no custom image selected, using default");
            None
        }
    } else {
        log::warn!("[suwayomi] sleep: directory {sleep_dir} does not exist, using default");
        None
    };

    // Double flash to clear heavy ghosting: flash to the inverse of the
    // background, wait, then write the actual sleep image and flash again.
    let bg = if SLEEP_BG_WHITE.load(Ordering::Relaxed) { 255u8 } else { 0u8 };
    let inverse_bg = if bg == 255 { 0u8 } else { 255u8 };
    let blank = vec![inverse_bg; fb.width as usize * fb.height as usize];
    for y in 0..fb.height as usize {
        fb.write_line(y, 0..fb.width as usize, &blank[y * fb.width as usize..(y + 1) * fb.width as usize]);
    }
    fb.refresh_full();
    fb.wait_for_update_complete();

    // Now write the actual sleep image
    let img = if let Some(path) = image_path {
        match image::open(&path) {
            Ok(img) => img.to_luma8(),
            Err(e) => {
                log::error!("[suwayomi] sleep: failed to decode {}: {e}, using default", path.display());
                generate_default_sleep_image(fb.width, fb.height)
            }
        }
    } else {
        generate_default_sleep_image(fb.width, fb.height)
    };

    let img = fit_to_screen(&img, fb.width, fb.height, bg);

    let img_raw: &[u8] = img.as_raw();
    let img_width = img.width() as usize;
    for y in 0..fb.height as usize {
        let row_pixels = &img_raw[y * img_width..(y + 1) * img_width];
        fb.write_line(y, 0..fb.width as usize, row_pixels);
    }

    fb.refresh_full();
    fb.wait_for_update_complete();
    log::info!("[suwayomi] sleep: displayed in {:.0}ms", start.elapsed().as_millis());
}

fn fit_to_screen(img: &image::GrayImage, screen_w: u32, screen_h: u32, bg: u8) -> image::GrayImage {
    use image::ImageBuffer;

    if img.width() == screen_w && img.height() == screen_h {
        return img.clone();
    }

    let scale = (screen_w as f64 / img.width() as f64)
        .min(screen_h as f64 / img.height() as f64);
    let new_w = ((img.width() as f64 * scale).round() as u32).max(1);
    let new_h = ((img.height() as f64 * scale).round() as u32).max(1);

    let resized = image::imageops::resize(img, new_w, new_h, image::imageops::FilterType::Nearest);

    let mut canvas: ImageBuffer<image::Luma<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(screen_w, screen_h, image::Luma([bg]));

    let offset_x = (screen_w - new_w) / 2;
    let offset_y = (screen_h - new_h) / 2;

    for y in 0..new_h {
        for x in 0..new_w {
            canvas.put_pixel(offset_x + x, offset_y + y, *resized.get_pixel(x, y));
        }
    }

    canvas
}

fn generate_default_sleep_image(width: u32, height: u32) -> image::GrayImage {
    use image::ImageBuffer;

    let mut img: ImageBuffer<image::Luma<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(width, height, image::Luma([0u8]));

    let cx = width as f32 * 0.5;
    let cy = height as f32 * 0.4;
    let moon_r = width as f32 * 0.12;
    let cutout_offset = moon_r * 0.45;
    let cutout_r = moon_r * 0.95;

    let stars = [
        (0.15, 0.20, 3.0f32), (0.82, 0.15, 2.5), (0.25, 0.55, 2.0),
        (0.75, 0.50, 2.5), (0.10, 0.75, 2.0), (0.90, 0.70, 3.0),
        (0.50, 0.80, 2.0), (0.35, 0.12, 1.5), (0.65, 0.25, 1.5),
        (0.05, 0.40, 1.5), (0.95, 0.35, 2.0), (0.45, 0.65, 1.5),
        (0.70, 0.85, 2.0), (0.20, 0.90, 1.5),
    ];

    for y in 0..height {
        for x in 0..width {
            let xf = x as f32;
            let yf = y as f32;

            let dx = xf - cx;
            let dy = yf - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            let cdx = xf - (cx + cutout_offset);
            let cdy = yf - cy;
            let cutout_dist = (cdx * cdx + cdy * cdy).sqrt();

            let in_moon = dist < moon_r;
            let in_cutout = cutout_dist < cutout_r;
            let in_crescent = in_moon && !in_cutout;

            let mut is_star = false;
            for &(sx, sy, sr) in &stars {
                let sdx = xf - sx * width as f32;
                let sdy = yf - sy * height as f32;
                let sdist = (sdx * sdx + sdy * sdy).sqrt();
                if sdist < sr {
                    is_star = true;
                    break;
                }
            }

            if in_crescent || is_star {
                img.put_pixel(x, y, image::Luma([255u8]));
            }
        }
    }

    img
}

fn screen_dimensions() -> (u32, u32) {
    let w = SCREEN_W.load(Ordering::Relaxed);
    let h = SCREEN_H.load(Ordering::Relaxed);
    if w > 0 && h > 0 {
        return (w, h);
    }
    match crate::framebuffer::Framebuffer::open() {
        Ok(fb) => (fb.width, fb.height),
        Err(_) => (1072, 1448),
    }
}

static PRE_RENDER_GEN: AtomicU32 = AtomicU32::new(0);

pub fn new_pre_render_generation() -> u32 {
    PRE_RENDER_GEN.fetch_add(1, Ordering::SeqCst) + 1
}

fn is_current_generation(generation: u32) -> bool {
    PRE_RENDER_GEN.load(Ordering::SeqCst) == generation
}

pub fn preload_sleep_screen_from_file(path: &std::path::Path) {
    let img = match image::open(path) {
        Ok(img) => img.to_luma8(),
        Err(e) => {
            log::error!("[suwayomi] preload_sleep: failed to decode {}: {e}", path.display());
            return;
        }
    };
    preload_sleep_screen_from_image(&img);
}

pub fn preload_sleep_screen_from_image(img: &image::GrayImage) {
    let start = std::time::Instant::now();
    let generation = new_pre_render_generation();
    let (w, h) = screen_dimensions();

    let bg = if SLEEP_BG_WHITE.load(Ordering::Relaxed) { 255u8 } else { 0u8 };
    let img = fit_to_screen(img, w, h, bg);

    if !is_current_generation(generation) {
        log::info!("[suwayomi] preload_sleep: superseded, discarding");
        return;
    }

    let cache = SleepScreenCache {
        pixels: img.as_raw().clone(),
        width: w,
        height: h,
    };

    *SLEEP_SCREEN_CACHE.lock().unwrap() = Some(cache);
    log::info!("[suwayomi] preload_sleep: cached in {:.0}ms", start.elapsed().as_millis());
}

pub fn clear_sleep_screen_cache() {
    *SLEEP_SCREEN_CACHE.lock().unwrap() = None;
}

pub fn preload_default_sleep_screen() {
    let generation = new_pre_render_generation();
    let (w, h) = screen_dimensions();
    let img = generate_default_sleep_image(w, h);

    if !is_current_generation(generation) {
        return;
    }

    let cache = SleepScreenCache {
        pixels: img.as_raw().clone(),
        width: w,
        height: h,
    };
    *SLEEP_SCREEN_CACHE.lock().unwrap() = Some(cache);
    log::info!("[suwayomi] preload_default_sleep: cached {}x{}", w, h);
}
