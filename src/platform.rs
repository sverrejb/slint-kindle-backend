use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType,
};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::Rgb8Pixel;

use crate::framebuffer::{Framebuffer, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use crate::touch::TouchInput;


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
}

impl KindlePlatform {
    pub(crate) fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(slint::PhysicalSize::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));
        Self { window, start: Instant::now() }
    }
}

impl Platform for KindlePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let mut fb = Framebuffer::open()
            .map_err(|e| PlatformError::Other(format!("failed to open /dev/fb0: {e}")))?;

        let mut touch = TouchInput::open()
            .map_err(|e| PlatformError::Other(format!("failed to open touch input: {e}")))?;

        fb.fill(0xff);
        fb.refresh_full();

        let mut rgb_scratch = vec![Rgb8Pixel::default(); DISPLAY_WIDTH as usize];
        let mut gray_scratch = vec![0u8; DISPLAY_WIDTH as usize];

        loop {
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

            // Cap sleep tighter during animations so they stay smooth, but never skip sleeping —
            // the E-ink panel can't refresh faster than its waveform (~150 ms), so a busy loop
            // just burns battery without producing extra frames.
            let max_sleep = if self.window.has_active_animations() {
                Duration::from_millis(33)
            } else {
                Duration::from_millis(100)
            };
            let sleep = slint::platform::duration_until_next_timer_update()
                .unwrap_or(max_sleep)
                .min(max_sleep);
            std::thread::sleep(sleep);
        }
    }
}
