use std::os::fd::AsRawFd;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::Rgb8Pixel;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{EventLoopProxy, Platform, PlatformError, WindowAdapter};

use crate::framebuffer::Framebuffer;
use crate::power::{arm_wakealarm, find_wakealarm, suspend_to_mem};
use crate::touch::TouchInput;
use crate::wakeup::{self, KindleEventLoopProxy, Queue, Wakeup};
use crate::{OnWakeCallback, WakeSchedule, REQUEST_FULL_REFRESH, WOKE_FROM_SUSPEND, get_rotation, get_render_offset};

// Animations get redrawn at most ~30 fps. E-ink can't keep up with anything
// faster, so quicker wakes would just waste battery.
const ANIMATION_FRAME: Duration = Duration::from_millis(33);

pub(crate) struct KindlePlatform {
    pub(crate) window: Rc<MinimalSoftwareWindow>,
    start: Instant,
    queue: Queue,
    wakeup: Wakeup,
    quit_flag: Arc<AtomicBool>,
    pub(crate) wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
    pub(crate) on_wake: OnWakeCallback,
    black_and_white: Arc<AtomicBool>,
    scale_factor: f32,
}

impl KindlePlatform {
    pub(crate) fn new(
        wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
        on_wake: OnWakeCallback,
        black_and_white: Arc<AtomicBool>,
        scale_factor: f32,
    ) -> std::io::Result<Self> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let wakeup = wakeup::make_wakeup()?;
        Ok(Self {
            window,
            start: Instant::now(),
            queue: Arc::new(Mutex::new(Vec::new())),
            wakeup,
            quit_flag: Arc::new(AtomicBool::new(false)),
            wake_schedule,
            on_wake,
            black_and_white,
            scale_factor,
        })
    }

    /// Suspend the device to RAM once it's been idle for `stay_awake` with no
    /// pending work, then arm the wakealarm to bring it back. Returns `true`
    /// if a suspend cycle ran (the caller should restart the event loop).
    fn suspend_if_idle(
        &self,
        frame_buffer: &Framebuffer,
        wakealarm: Option<&Path>,
        last_interaction: &mut Instant,
    ) -> bool {
        let (Some(schedule), Some(wakealarm_path)) = (
            *self.wake_schedule.lock().expect("wake schedule poisoned"),
            wakealarm,
        ) else {
            return false;
        };

        // Pending Slint timers don't block suspend: they'll just fire on
        // resume (a 1 Hz clock timer would otherwise pin the device awake).
        let nothing_pending = !self.window.has_active_animations()
            && self
                .queue
                .lock()
                .expect("event loop closure queue poisoned")
                .is_empty();
        if last_interaction.elapsed() < schedule.stay_awake || !nothing_pending {
            return false;
        }

        frame_buffer.wait_for_update_complete();

        // If arming fails we still suspend, sleeping to save battery is better than
        // staying awake.
        if let Err(e) = arm_wakealarm(wakealarm_path, schedule.wake_interval) {
            log::error!(
                "failed to arm RTC wakealarm: {e}; device may only wake on user input this cycle"
            );
        }
        if let Err(e) = suspend_to_mem() {
            log::error!("suspend-to-RAM failed: {e}");
        }

        // Start a fresh stay_awake window so the consumer's app
        // gets at least that long to react.
        *last_interaction = Instant::now();
        // Fire the consumer's on-wake callback (if any) before any rendering
        // this cycle, so e.g. an HTTP poll runs before the next draw shows
        // stale data.
        if let Some(callback) = self.on_wake.borrow_mut().as_mut() {
            callback();
        }
        true
    }
}

impl Platform for KindlePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(KindleEventLoopProxy {
            queue: self.queue.clone(),
            write_fd: self.wakeup.write.clone(),
            quit_flag: self.quit_flag.clone(),
        }))
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let mut frame_buffer = Framebuffer::open()
            .map_err(|e| PlatformError::Other(format!("failed to open /dev/fb0: {e}")))?;

        let sf = self.scale_factor;
        let fb_w = frame_buffer.width as usize;
        let fb_h = frame_buffer.height as usize;

        // Read rotation and compute effective render dimensions.
        // For 0°/180°: render at framebuffer native size (portrait).
        // For 90°/270°: render at swapped size (landscape) so Slint lays
        // out the UI in landscape, then we transpose when writing to fb.
        let rotation = get_rotation();
        let (render_w, render_h) = match rotation {
            90 | 270 => (fb_h, fb_w),
            _ => (fb_w, fb_h),
        };

        self.window
            .set_size(slint::PhysicalSize::new(render_w as u32, render_h as u32));

        let mut touch_input = TouchInput::open(frame_buffer.width, frame_buffer.height, sf)
            .map_err(|e| PlatformError::Other(format!("failed to open touch input: {e}")))?;

        frame_buffer.fill(0xff);
        frame_buffer.refresh_full();

        let width = render_w;
        let mut rgb_buffer = vec![Rgb8Pixel::default(); width * render_h];
        let mut gray_buffer = vec![0u8; width];

        // Track the rotation used for the last full render. When the rotation
        // changes at runtime, we must re-render the entire screen (not just
        // dirty regions) because every pixel's framebuffer position changes.
        let mut last_rendered_rotation = (rotation + get_render_offset()) % 360;

        let wakeup_read_fd = self.wakeup.read.as_raw_fd();

        // Wakealarm path is probed once. If the device doesn't expose one
        // (e.g. running on a dev host), the suspend cycle stays disabled even
        // if a schedule is configured.
        let wakealarm = find_wakealarm().ok();
        let mut last_interaction = Instant::now();

        loop {
            // A suspend cycle restarts the loop with a fresh stay-awake window.
            if self.suspend_if_idle(&frame_buffer, wakealarm.as_deref(), &mut last_interaction) {
                continue;
            }

            // Wait for touch event or wakeup from application thread.
            // -1 means "wait forever," which lets the CPU go to sleep.
            let timeout_ms: libc::c_int = match (
                self.window.has_active_animations(),
                slint::platform::duration_until_next_timer_update(),
            ) {
                (true, Some(d)) => duration_to_ms(d.min(ANIMATION_FRAME)),
                (true, None) => duration_to_ms(ANIMATION_FRAME),
                (false, Some(d)) => duration_to_ms(d),
                (false, None) => -1,
            };

            // [0] - touch events file descriptor
            // [1] - wakeup pipe for userland application threads
            let mut file_descriptors = [
                libc::pollfd {
                    fd: touch_input.fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wakeup_read_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            // Block until an fd has activity or the timeout expires.
            // Retry on EINTR, bail on any other error.
            // SAFETY: fds is a valid 2-element array while poll runs.
            let poll_result = unsafe {
                libc::poll(
                    file_descriptors.as_mut_ptr(),
                    file_descriptors.len() as libc::nfds_t,
                    timeout_ms,
                )
            };
            if poll_result < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(PlatformError::Other(format!("poll failed: {err}")));
            }

            // Bail if either file descriptor has died to avoid waiting forever on input
            let err_bits = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
            if (file_descriptors[0].revents | file_descriptors[1].revents) & err_bits != 0 {
                return Err(PlatformError::Other(format!(
                    "poll: input fd died (touch revents={:#x}, wakeup revents={:#x})",
                    file_descriptors[0].revents, file_descriptors[1].revents
                )));
            }

            // Empty the pipe before running closures so any new wakeup that arrives
            // while a closure runs still triggers another loop iteration.
            if file_descriptors[1].revents & libc::POLLIN != 0 {
                wakeup::drain(&self.wakeup.read);
                let pending: Vec<_> = self
                    .queue
                    .lock()
                    .expect("event loop closure queue poisoned")
                    .drain(..)
                    .collect();
                for c in pending {
                    c();
                }
            }

            // Check early for quit before doing more work
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }

            // Touch activity counts as user interaction, so it resets the
            // suspend countdown
            if file_descriptors[0].revents & libc::POLLIN != 0 {
                last_interaction = Instant::now();
            }

            touch_input.poll(&self.window);
            slint::platform::update_timers_and_animations();

            let black_and_white = self.black_and_white.load(Ordering::Relaxed);
            let full_refresh = REQUEST_FULL_REFRESH.swap(false, Ordering::Relaxed);

            // Apply the fixed render offset for this session.
            // On the Kindle Oasis, the framebuffer's hardware rotation is
            // set by the framework before we take over. If we launched at
            // 180°, the fb is already rotated, so we add 180° to every
            // render. This offset is fixed at launch and never changes.
            let current_rotation = get_rotation();
            let render_rotation = (current_rotation + get_render_offset()) % 360;
            let rotation_changed = render_rotation != last_rendered_rotation;
            if rotation_changed {
                last_rendered_rotation = render_rotation;
                self.window.request_redraw();
            }

            // After wake from suspend, force a redraw so the stale sleep
            // image is replaced with fresh app content.
            if WOKE_FROM_SUSPEND.swap(false, Ordering::Relaxed) {
                self.window.request_redraw();
            }

            let mut did_draw = false;
            self.window.draw_if_needed(|renderer| {
                did_draw = true;
                let dirty = renderer.render(&mut rgb_buffer, width);
                let origin = dirty.bounding_box_origin();
                let size = dirty.bounding_box_size();
                let (x0, y0) = (origin.x as usize, origin.y as usize);
                let (w, h) = (size.width as usize, size.height as usize);
                if w == 0 || h == 0 {
                    return;
                }

                let gray = &mut gray_buffer[..w];
                for row in 0..h {
                    let start = (y0 + row) * width + x0;
                    let rgb = &rgb_buffer[start..start + w];
                    for (g, p) in gray.iter_mut().zip(rgb.iter()) {
                        let value =
                            ((77 * p.r as u32 + 150 * p.g as u32 + 29 * p.b as u32) >> 8) as u8;
                        *g = if black_and_white {
                            if value < 128 { 0x00 } else { 0xff }
                        } else {
                            value
                        };
                    }
                    write_row_rotated_range(
                        &mut frame_buffer, y0 + row, x0, &gray, render_rotation,
                        fb_w, fb_h, render_w, render_h,
                    );
                }

                if full_refresh || rotation_changed {
                    frame_buffer.refresh_full();
                } else {
                    let (fb_origin, fb_size) = match render_rotation {
                        180 => {
                            let fb_x0 = fb_w.saturating_sub(x0 + w);
                            let fb_y0 = fb_h - 1 - (y0 + h - 1);
                            (
                                slint::PhysicalPosition::new(fb_x0 as i32, fb_y0 as i32),
                                slint::PhysicalSize::new(w as u32, h as u32),
                            )
                        }
                        90 => {
                            let fb_x0 = fb_w - 1 - (y0 + h - 1);
                            let fb_y0 = x0;
                            (
                                slint::PhysicalPosition::new(fb_x0 as i32, fb_y0 as i32),
                                slint::PhysicalSize::new(h as u32, w as u32),
                            )
                        }
                        270 => {
                            let fb_x0 = y0;
                            let fb_y0 = fb_h - 1 - (x0 + w - 1);
                            (
                                slint::PhysicalPosition::new(fb_x0 as i32, fb_y0 as i32),
                                slint::PhysicalSize::new(h as u32, w as u32),
                            )
                        }
                        _ => (origin, size),
                    };
                    frame_buffer.refresh_region(fb_origin, fb_size);
                }
            });

            // If a full refresh was requested or rotation changed, force a
            // full reblit. draw_if_needed only writes the DIRTY region to the
            // framebuffer — the rest would keep stale content (e.g. the sleep
            // image after wake). full_reblit writes the entire rgb_buffer.
            if full_refresh || rotation_changed {
                full_reblit(&mut frame_buffer, &rgb_buffer, &mut gray_buffer,
                    black_and_white, render_rotation, fb_w, fb_h, render_w, render_h);
            }
        }

        Ok(())
    }
}

fn duration_to_ms(d: Duration) -> libc::c_int {
    // Round up to at least 1 ms. A timeout of 0 makes poll skip the wait
    // entirely, which would spin the CPU if a tiny timer kept re-firing.
    d.as_millis().clamp(1, libc::c_int::MAX as u128) as libc::c_int
}

// ---------------------------------------------------------------------------
// Rotation helpers
// ---------------------------------------------------------------------------

/// Write the entire render buffer to the framebuffer with the given rotation,
/// converting RGB to grayscale, then do a full refresh.
///
/// Used after rotation changes (every pixel's framebuffer position changes)
/// and after waking from suspend (framebuffer has stale sleep image).
fn full_reblit(
    fb: &mut Framebuffer,
    rgb_buffer: &[Rgb8Pixel],
    gray_buffer: &mut [u8],
    black_and_white: bool,
    rotation: u32,
    fb_w: usize,
    fb_h: usize,
    render_w: usize,
    render_h: usize,
) {
    let width = render_w;
    let gray = &mut gray_buffer[..width];
    for row in 0..render_h {
        let rgb = &rgb_buffer[row * width..(row + 1) * width];
        for (g, p) in gray.iter_mut().zip(rgb.iter()) {
            let value = ((77 * p.r as u32 + 150 * p.g as u32 + 29 * p.b as u32) >> 8) as u8;
            *g = if black_and_white {
                if value < 128 { 0x00 } else { 0xff }
            } else {
                value
            };
        }
        write_row_rotated_range(fb, row, 0, gray, rotation, fb_w, fb_h, render_w, render_h);
    }
    fb.refresh_full();
}

/// Write a partial row (range x0..x0+len) to the framebuffer with rotation.
/// `pixels` contains only the dirty range, not the full row.
fn write_row_rotated_range(
    fb: &mut Framebuffer,
    row: usize,
    x0: usize,
    pixels: &[u8],
    rotation: u32,
    fb_w: usize,
    fb_h: usize,
    render_w: usize,
    render_h: usize,
) {
    let _ = (render_w, render_h);
    let len = pixels.len();
    match rotation {
        0 => {
            fb.write_line(row, x0..x0 + len, pixels);
        }
        180 => {
            let fb_row = fb_h - 1 - row;
            let fb_x0 = fb_w.saturating_sub(x0 + len);
            let mut reversed = pixels.to_vec();
            reversed.reverse();
            fb.write_line(fb_row, fb_x0..fb_x0 + len, &reversed);
        }
        90 => {
            let fb_x = fb_w - 1 - row;
            for (i, &val) in pixels.iter().enumerate() {
                let rx = x0 + i;
                fb.write_pixel(fb_x, rx, val);
            }
        }
        270 => {
            let fb_x = row;
            for (i, &val) in pixels.iter().enumerate() {
                let rx = x0 + i;
                let fb_y = fb_h - 1 - rx;
                fb.write_pixel(fb_x, fb_y, val);
            }
        }
        _ => {
            fb.write_line(row, x0..x0 + len, pixels);
        }
    }
}
