//! Linux framebuffer interface for Kindle e-ink displays.
//!
//! Opens `/dev/fb0`, queries the kernel for the actual screen geometry (so we
//! work on any Kindle model), mmaps the pixel buffer, and issues EPDC refresh
//! ioctls to push changes to the e-ink panel.

use std::ops::Range;
use std::os::fd::AsRawFd;

// Standard Linux framebuffer ioctl numbers (see <linux/fb.h>).
const FBIOGET_VSCREENINFO: libc::Ioctl = 0x4600;
const FBIOGET_FSCREENINFO: libc::Ioctl = 0x4602;

// These structs mirror the kernel's `fb_var_screeninfo` and `fb_fix_screeninfo`.
// We only read from them - the fields we care about are `xres`, `yres` (visible
// resolution) and `line_length` (stride in bytes per row, which may be larger
// than xres due to alignment padding).

#[repr(C)]
#[derive(Default)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Default)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

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

// Kindle EPDC (Electrophoretic Display Controller) ioctl and constants.
// The ioctl number was confirmed by stracing `eips` on a real device.
const MXCFB_SEND_UPDATE: libc::Ioctl = 0x4048_462e;

const WAVEFORM_MODE_GC16: u32 = 2; // Full 16-level grayscale refresh (slow, high quality)
const WAVEFORM_MODE_AUTO: u32 = 257; // Let the driver pick the best waveform
const UPDATE_MODE_PARTIAL: u32 = 0; // Only redraw the dirty region
const UPDATE_MODE_FULL: u32 = 1; // Flash the whole screen (clears ghosting)
const TEMP_USE_AMBIENT: i32 = 0x1000; // Use the panel's ambient temperature sensor

/// Memory-mapped handle to the Kindle's e-ink framebuffer.
///
/// Pixel format is 8-bit grayscale (one byte per pixel). The `stride` may be
/// wider than `width` due to hardware alignment requirements.
pub(crate) struct Framebuffer {
    file: std::fs::File,
    map: *mut u8,
    len: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Bytes per row in the mmap (≥ width due to padding).
    stride: usize,
}

// SAFETY: The mmap is process-wide and we only access it from the event loop thread.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    /// Open the framebuffer device and query its geometry from the kernel.
    ///
    /// This works on any Kindle model - the resolution and stride are read at
    /// runtime rather than being hardcoded.
    pub(crate) fn open() -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")?;

        let fd = file.as_raw_fd();

        let mut vinfo = FbVarScreeninfo::default();
        if unsafe {
            libc::ioctl(
                fd,
                FBIOGET_VSCREENINFO,
                &mut vinfo as *mut _ as *mut libc::c_void,
            )
        } == -1
        {
            return Err(std::io::Error::last_os_error());
        }

        let mut finfo = FbFixScreeninfo::default();
        if unsafe {
            libc::ioctl(
                fd,
                FBIOGET_FSCREENINFO,
                &mut finfo as *mut _ as *mut libc::c_void,
            )
        } == -1
        {
            return Err(std::io::Error::last_os_error());
        }

        let width = vinfo.xres;
        let height = vinfo.yres;
        let stride = finfo.line_length as usize;

        if width == 0 || height == 0 || stride < width as usize {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid framebuffer geometry: {width}x{height}, stride={stride}"),
            ));
        }

        let len = stride * height as usize;

        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };

        if map == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self {
            file,
            map: map as *mut u8,
            len,
            width,
            height,
            stride,
        })
    }

    /// Write a horizontal span of grayscale pixels into the mmap at row `y`.
    pub(crate) fn write_line(&mut self, y: usize, x_range: Range<usize>, pixels: &[u8]) {
        let dst = unsafe {
            std::slice::from_raw_parts_mut(
                self.map.add(y * self.stride + x_range.start),
                pixels.len(),
            )
        };
        dst.copy_from_slice(pixels);
    }

    /// Fill the entire visible area with a single grayscale value (0x00 = black, 0xff = white).
    pub(crate) fn fill(&mut self, value: u8) {
        for y in 0..self.height as usize {
            let dst = unsafe {
                std::slice::from_raw_parts_mut(self.map.add(y * self.stride), self.width as usize)
            };
            dst.fill(value);
        }
    }

    /// Ask the EPDC to refresh a region of the e-ink panel.
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
                update_region: UpdateRect {
                    top: 0,
                    left: 0,
                    width: 0,
                    height: 0,
                },
            },
        };

        unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                MXCFB_SEND_UPDATE,
                &update as *const _,
            );
        }
    }

    /// Full-screen GC16 refresh - flashes the display to clear ghosting.
    pub(crate) fn refresh_full(&self) {
        self.send_update(
            UpdateRect {
                top: 0,
                left: 0,
                width: self.width,
                height: self.height,
            },
            WAVEFORM_MODE_GC16,
            UPDATE_MODE_FULL,
        );
    }

    /// Partial refresh of a dirty rectangle - fast, but may leave faint ghosting.
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
