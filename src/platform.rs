use std::os::fd::AsRawFd;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::Rgb8Pixel;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RenderingRotation, RepaintBufferType,
};
use slint::platform::{EventLoopProxy, Platform, PlatformError, WindowAdapter};

use crate::framebuffer::Framebuffer;
use crate::power::{arm_wakealarm, find_wakealarm, suspend_to_mem};
use crate::touch::{TouchInput, has_accelerometer};
use crate::wakeup::{self, KindleEventLoopProxy, Queue, Wakeup};
use crate::{OnWakeCallback, WakeSchedule};

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
    rotation: Arc<AtomicU32>,
}

impl KindlePlatform {
    pub(crate) fn new(
        wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
        on_wake: OnWakeCallback,
        black_and_white: Arc<AtomicBool>,
        rotation: Arc<AtomicU32>,
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
            rotation,
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

        let mut touch_input = TouchInput::open(frame_buffer.width, frame_buffer.height)
            .map_err(|e| PlatformError::Other(format!("failed to open touch input: {e}")))?;

        frame_buffer.fill(0xff);
        frame_buffer.refresh_full();

        let width = frame_buffer.width as usize;
        let mut rgb_buffer = vec![Rgb8Pixel::default(); width * frame_buffer.height as usize];
        let mut gray_buffer = vec![0u8; width];

        let render_offset = launch_render_offset();
        let add_launch_offset = |app_rotation: u32| (app_rotation + render_offset) % 360;
        // 90 or 270 rotated window is landscape mode
        let (panel_width, panel_height) = (frame_buffer.width, frame_buffer.height);
        let window_size = |app_rotation: u32| {
            if add_launch_offset(app_rotation).is_multiple_of(180) {
                slint::PhysicalSize::new(panel_width, panel_height)
            } else {
                slint::PhysicalSize::new(panel_height, panel_width)
            }
        };
        let mut applied_rotation = self.rotation.load(Ordering::Relaxed);
        self.window.set_size(window_size(applied_rotation));

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

            touch_input.poll(&self.window, applied_rotation);
            slint::platform::update_timers_and_animations();

            let black_and_white = self.black_and_white.load(Ordering::Relaxed);

            let app_rotation = self.rotation.load(Ordering::Relaxed);
            let rotation_changed = app_rotation != applied_rotation;
            if rotation_changed {
                applied_rotation = app_rotation;
                self.window.set_size(window_size(app_rotation));
                self.window.request_redraw();
            }
            self.window.draw_if_needed(|renderer| {
                renderer.set_rendering_rotation(match add_launch_offset(applied_rotation) {
                    90 => RenderingRotation::Rotate90,
                    180 => RenderingRotation::Rotate180,
                    270 => RenderingRotation::Rotate270,
                    _ => RenderingRotation::NoRotation,
                });
                if rotation_changed {
                    renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
                    renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
                }
                let dirty = renderer.render(&mut rgb_buffer, width);
                let origin = dirty.bounding_box_origin();
                let size = dirty.bounding_box_size();
                let (x0, y0) = (origin.x as usize, origin.y as usize);
                let (w, h) = (size.width as usize, size.height as usize);

                let gray = &mut gray_buffer[..w];
                for row in 0..h {
                    let start = (y0 + row) * width + x0;
                    let rgb = &rgb_buffer[start..start + w];
                    for (g, p) in gray.iter_mut().zip(rgb.iter()) {
                        let value =
                            ((77 * p.r as u32 + 150 * p.g as u32 + 29 * p.b as u32) >> 8) as u8;
                        // Black-and-white mode forces pure black/white based on threshold
                        *g = if black_and_white {
                            if value < 128 { 0x00 } else { 0xff }
                        } else {
                            value
                        };
                    }
                    frame_buffer.write_line(y0 + row, x0..x0 + w, gray);
                }

                // A rotation change rewrites every pixel so we refresh the whole panel
                // so no ghost of the old orientation survives.
                if rotation_changed {
                    frame_buffer.refresh_full();
                } else {
                    frame_buffer.refresh_region(origin, size);
                }
            });
        }

        Ok(())
    }
}

fn duration_to_ms(d: Duration) -> libc::c_int {
    // Round up to at least 1 ms. A timeout of 0 makes poll skip the wait
    // entirely, which would spin the CPU if a tiny timer kept re-firing.
    d.as_millis().clamp(1, libc::c_int::MAX as u128) as libc::c_int
}

/// Devices with an accelerometer let the framework flip its UI 180° when
/// held the other way up. Touch input then already arrives flipped, but the
/// framebuffer does not, so rendering has to compensate by whatever the
/// framework thinks is up when the app starts. 
fn launch_render_offset() -> u32 {
    let flipped = has_accelerometer()
        && std::process::Command::new("lipc-get-prop")
            .args(["com.lab126.winmgr", "accelerometer"])
            .output()
            .is_ok_and(|out| out.status.success() && out.stdout.trim_ascii() == b"D");
    if flipped { 180 } else { 0 }
}
