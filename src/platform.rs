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


struct KindleLineBuffer<'fb> {
    fb: &'fb mut Framebuffer,
    scratch: Vec<Rgb8Pixel>,
}

impl<'fb> KindleLineBuffer<'fb> {
    fn new(fb: &'fb mut Framebuffer) -> Self {
        Self { fb, scratch: vec![Rgb8Pixel::default(); DISPLAY_WIDTH as usize] }
    }
}

impl LineBufferProvider for KindleLineBuffer<'_> {
    type TargetPixel = Rgb8Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        let buf = &mut self.scratch[range.clone()];
        render_fn(buf);

        // The E-ink screen only shows grayscale, so turn each RGB pixel into a single gray value.
        let gray: Vec<u8> = buf
            .iter()
            .map(|p| (0.299 * p.r as f32 + 0.587 * p.g as f32 + 0.114 * p.b as f32) as u8)
            .collect();

        self.fb.write_line(line, range, &gray);
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

        loop {
            touch.poll(&self.window);

            slint::platform::update_timers_and_animations();

            self.window.draw_if_needed(|renderer| {
                let dirty = renderer.render_by_line(KindleLineBuffer::new(&mut fb));
                fb.refresh_region(dirty.bounding_box_origin(), dirty.bounding_box_size());
            });

            if !self.window.has_active_animations() {
                let sleep = slint::platform::duration_until_next_timer_update()
                    .unwrap_or(Duration::from_millis(16));
                std::thread::sleep(sleep.min(Duration::from_millis(16)));
            }
        }
    }
}
