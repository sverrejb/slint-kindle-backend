use std::ops::Range;
use std::os::fd::AsRawFd;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::Rgb8Pixel;
use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType,
};
use slint::platform::{EventLoopProxy, Platform, PlatformError, WindowAdapter};

use crate::framebuffer::Framebuffer;
use crate::touch::TouchInput;
use crate::wakeup::{self, KindleEventLoopProxy, Queue, Wakeup};

// Animations get redrawn at most ~30 fps. E-ink can't keep up with anything
// faster, so quicker wakes would just waste battery.
const ANIMATION_FRAME: Duration = Duration::from_millis(33);

struct KindleLineBuffer<'a> {
    fb: &'a mut Framebuffer,
    rgb_scratch: &'a mut [Rgb8Pixel],
    gray_scratch: &'a mut [u8],
}

impl LineBufferProvider for KindleLineBuffer<'_> {
    type TargetPixel = Rgb8Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        let rgb = &mut self.rgb_scratch[range.clone()];
        render_fn(rgb);

        // The E-ink screen only shows grayscale, so turn each RGB pixel into a single gray value.
        // BT.601 luma weights (0.299, 0.587, 0.114) scaled by 256 — sum is 256 so the divide is a shift.
        let gray = &mut self.gray_scratch[range.clone()];
        for (g, p) in gray.iter_mut().zip(rgb.iter()) {
            *g = ((77 * p.r as u32 + 150 * p.g as u32 + 29 * p.b as u32) >> 8) as u8;
        }

        self.fb.write_line(line, range, gray);
    }
}

pub(crate) struct KindlePlatform {
    pub(crate) window: Rc<MinimalSoftwareWindow>,
    start: Instant,
    queue: Queue,
    wakeup: Wakeup,
    quit_flag: Arc<AtomicBool>,
}

impl KindlePlatform {
    pub(crate) fn new() -> std::io::Result<Self> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let wakeup = wakeup::make_wakeup()?;
        Ok(Self {
            window,
            start: Instant::now(),
            queue: Arc::new(Mutex::new(Vec::new())),
            wakeup,
            quit_flag: Arc::new(AtomicBool::new(false)),
        })
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
        let mut fb = Framebuffer::open()
            .map_err(|e| PlatformError::Other(format!("failed to open /dev/fb0: {e}")))?;

        self.window
            .set_size(slint::PhysicalSize::new(fb.width, fb.height));

        let mut touch = TouchInput::open(fb.width, fb.height)
            .map_err(|e| PlatformError::Other(format!("failed to open touch input: {e}")))?;

        fb.fill(0xff);
        fb.refresh_full();

        let mut rgb_scratch = vec![Rgb8Pixel::default(); fb.width as usize];
        let mut gray_scratch = vec![0u8; fb.width as usize];

        let wakeup_read_fd = self.wakeup.read.as_raw_fd();

        loop {
            // Wait for something to happen like a touch, a wakeup poke, or a timer.
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

            let mut fds = [
                libc::pollfd {
                    fd: touch.fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wakeup_read_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            // SAFETY: fds is a valid 2-element array while poll runs.
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(PlatformError::Other(format!("poll failed: {err}")));
            }

            // If either fd has died, bail — otherwise poll keeps returning instantly
            // and we'd burn the CPU forever waiting for input that's never coming.
            let err_bits = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
            if (fds[0].revents | fds[1].revents) & err_bits != 0 {
                return Err(PlatformError::Other(format!(
                    "poll: input fd died (touch revents={:#x}, wakeup revents={:#x})",
                    fds[0].revents, fds[1].revents
                )));
            }

            // Empty the pipe before running closures so any new wakeup that arrives
            // while a closure runs still triggers another loop iteration.
            if fds[1].revents & libc::POLLIN != 0 {
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

            // Check for quit before doing more work — a screen refresh takes a
            // noticeable chunk of time on E-ink and we'd rather skip it on the way out.
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }

            touch.poll(&self.window);
            slint::platform::update_timers_and_animations();

            self.window.draw_if_needed(|renderer| {
                let dirty = renderer.render_by_line(KindleLineBuffer {
                    fb: &mut fb,
                    rgb_scratch: &mut rgb_scratch,
                    gray_scratch: &mut gray_scratch,
                });
                fb.refresh_region(dirty.bounding_box_origin(), dirty.bounding_box_size());
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
