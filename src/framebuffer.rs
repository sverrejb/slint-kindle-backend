use std::ops::Range;
use std::os::fd::AsRawFd;

pub(crate) const DISPLAY_WIDTH: u32 = 1072;
pub(crate) const DISPLAY_HEIGHT: u32 = 1448;
// Each row is 1088 bytes wide in memory even though only 1072 pixels show — the rest is padding.
const FB_STRIDE: usize = 1088;


#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct UpdateRect {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AlternateBuffer {
    physical_address: u32,
    width: u32,
    height: u32,
    update_region: UpdateRect,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UpdateRequest {
    update_region: UpdateRect,
    waveform_mode: u32,
    update_mode: u32,
    update_marker: u32,
    previous_bw_waveform_mode: u32,
    previous_gray_waveform_mode: u32,
    temperature: i32,
    flags: u32,
    alternate_buffer: AlternateBuffer,
}

// Magic number the Kindle's display driver uses to mean "redraw this part of the screen".
const MXCFB_SEND_UPDATE: libc::c_ulong = 0x4048_462e;

const WAVEFORM_MODE_GC16: u32 = 2;
const WAVEFORM_MODE_AUTO: u32 = 257;
const UPDATE_MODE_PARTIAL: u32 = 0;
const UPDATE_MODE_FULL: u32 = 1;
const TEMP_USE_AMBIENT: i32 = 0x1000;


pub(crate) struct Framebuffer {
    file: std::fs::File,
    map: *mut u8,
    len: usize,
}

unsafe impl Send for Framebuffer {}

impl Framebuffer {
    pub(crate) fn open() -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")?;

        let len = FB_STRIDE * DISPLAY_HEIGHT as usize;

        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };

        if map == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self { file, map: map as *mut u8, len })
    }

    pub(crate) fn write_line(&mut self, y: usize, x_range: Range<usize>, pixels: &[u8]) {
        let dst = unsafe {
            std::slice::from_raw_parts_mut(
                self.map.add(y * FB_STRIDE + x_range.start),
                pixels.len(),
            )
        };
        dst.copy_from_slice(pixels);
    }

    pub(crate) fn fill(&mut self, value: u8) {
        for y in 0..DISPLAY_HEIGHT as usize {
            let dst = unsafe {
                std::slice::from_raw_parts_mut(
                    self.map.add(y * FB_STRIDE),
                    DISPLAY_WIDTH as usize,
                )
            };
            dst.fill(value);
        }
    }

    fn send_update(&self, region: UpdateRect, waveform: u32, mode: u32) {
        let update = UpdateRequest {
            update_region: region,
            waveform_mode: waveform,
            update_mode: mode,
            update_marker: 1,
            previous_bw_waveform_mode: 0,
            previous_gray_waveform_mode: 0,
            temperature: TEMP_USE_AMBIENT,
            flags: 0,
            alternate_buffer: AlternateBuffer {
                physical_address: 0,
                width: 0,
                height: 0,
                update_region: UpdateRect { top: 0, left: 0, width: 0, height: 0 },
            },
        };

        unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                MXCFB_SEND_UPDATE as _,
                &update as *const _,
            );
        }
    }

    pub(crate) fn refresh_full(&self) {
        self.send_update(
            UpdateRect { top: 0, left: 0, width: DISPLAY_WIDTH, height: DISPLAY_HEIGHT },
            WAVEFORM_MODE_GC16,
            UPDATE_MODE_FULL,
        );
    }

    pub(crate) fn refresh_region(
        &self,
        origin: slint::PhysicalPosition,
        size: slint::PhysicalSize,
    ) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.send_update(
            UpdateRect {
                top: origin.y as u32,
                left: origin.x as u32,
                width: size.width,
                height: size.height,
            },
            WAVEFORM_MODE_AUTO,
            UPDATE_MODE_PARTIAL,
        );
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.map as *mut libc::c_void, self.len) };
    }
}
