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
mod touch;

use platform::KindlePlatform;
use slint::platform::WindowAdapter;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use std::rc::Rc;

/// Returned by [`install`]. Use it to add more fonts later.
pub struct KindleBackend {
    window: Rc<MinimalSoftwareWindow>,
}

impl KindleBackend {
    /// Add an extra font (TTF/OTF) from bytes.
    ///
    /// Call this **after** you've created your window (e.g. `AppWindow::new()`).
    /// Fonts can't be added before then because Slint hasn't set up its font
    /// system yet.
    pub fn register_font_from_memory(&self, data: &'static [u8]) -> Result<(), slint::PlatformError> {
        self.window
            .renderer()
            .register_font_from_memory(data)
            .map_err(|e| slint::PlatformError::Other(format!("{e}")))
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
    let path = std::env::temp_dir().join("slint-kindle-default.ttf");
    std::fs::write(&path, font_data)
        .map_err(|e| slint::PlatformError::Other(format!("failed to stage default font: {e}")))?;

    // SAFETY: install() runs once at startup before any threads exist, so nothing else can read this env var at the same time.
    unsafe { std::env::set_var("SLINT_DEFAULT_FONT", &path); }

    let platform = KindlePlatform::new();
    let window = platform.window.clone();
    slint::platform::set_platform(Box::new(platform))
        .map_err(|e| slint::PlatformError::Other(format!("{e}")))?;
    Ok(KindleBackend { window })
}
