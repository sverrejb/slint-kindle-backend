mod ffi;

use std::cell::Cell;
use std::ops::Range;
use std::os::fd::AsRawFd;

use ffi::{
    AlternateBuffer, FBIOGET_FSCREENINFO, FBIOGET_VSCREENINFO, FbFixScreeninfo, FbVarScreeninfo,
    MXCFB_SEND_UPDATE, MXCFB_SEND_UPDATE_REX, MXCFB_WAIT_FOR_UPDATE_COMPLETE, TEMP_USE_AMBIENT,
    UPDATE_MODE_FULL, UPDATE_MODE_PARTIAL, UpdateMarkerData, UpdateRect, UpdateRequest,
    UpdateRequestRex, WAVEFORM_MODE_AUTO, WAVEFORM_MODE_GC16,
};

/// Which MXCFB update ioctl this kernel accepts.
///
/// The two generations can't be told apart a priori — the framebuffer's driver
/// id string is `mxc_epdc_fb` on both, and the difference is the update struct's
/// size (72 vs 80 bytes), which nothing advertises. So we probe on first refresh
/// and remember the winner instead of retrying the failing ioctl every frame.
#[derive(Clone, Copy)]
enum UpdateVariant {
    /// `MXCFB_SEND_UPDATE` — 72-byte struct, older devices.
    Legacy,
    /// `MXCFB_SEND_UPDATE_REX` — 80-byte struct, Paperwhite 10th gen and newer.
    Rex,
}

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
    stride: usize,
    /// The update ioctl variant this kernel accepts, cached after the first
    /// successful refresh. `None` until then.
    update_variant: Cell<Option<UpdateVariant>>,
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
                FBIOGET_VSCREENINFO as _,
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
                FBIOGET_FSCREENINFO as _,
                &mut finfo as *mut _ as *mut libc::c_void,
            )
        } == -1
        {
            return Err(std::io::Error::last_os_error());
        }

        let width = vinfo.xres;
        let height = vinfo.yres;
        let stride = finfo.line_length as usize;

        // The whole render path treats the mmap as one byte per pixel. A
        // different depth would silently produce garbled output, so reject it
        // with a clear error instead.
        if vinfo.bits_per_pixel != 8 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "unsupported framebuffer depth: {} bpp (expected 8-bit grayscale)",
                    vinfo.bits_per_pixel
                ),
            ));
        }

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
            update_variant: Cell::new(None),
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
    ///
    /// On the first call we probe which update ioctl the kernel accepts and
    /// cache it, so later frames issue exactly one ioctl instead of retrying a
    /// known-failing one every refresh.
    fn send_update(&self, region: UpdateRect, waveform: u32, mode: u32) {
        match self.update_variant.get() {
            Some(UpdateVariant::Legacy) => {
                self.send_update_legacy(region, waveform, mode);
            }
            Some(UpdateVariant::Rex) => {
                self.send_update_rex(region, waveform, mode);
            }
            None => {
                if self.send_update_legacy(region, waveform, mode) {
                    self.update_variant.set(Some(UpdateVariant::Legacy));
                } else if self.send_update_rex(region, waveform, mode) {
                    self.update_variant.set(Some(UpdateVariant::Rex));
                } else {
                    log::error!(
                        "EPDC refresh failed: neither MXCFB_SEND_UPDATE nor \
                         MXCFB_SEND_UPDATE_REX was accepted; the screen will not update"
                    );
                }
            }
        }
    }

    /// Issue the legacy `MXCFB_SEND_UPDATE` (72-byte struct). Returns whether the
    /// ioctl succeeded.
    fn send_update_legacy(&self, region: UpdateRect, waveform: u32, mode: u32) -> bool {
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
        // SAFETY: `update` outlives the ioctl and matches the kernel's struct.
        unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                MXCFB_SEND_UPDATE as _,
                &update as *const _,
            ) != -1
        }
    }

    /// Issue the modern `MXCFB_SEND_UPDATE_REX` (80-byte struct). Returns whether
    /// the ioctl succeeded.
    fn send_update_rex(&self, region: UpdateRect, waveform: u32, mode: u32) -> bool {
        let update = UpdateRequestRex {
            update_region: region,
            waveform_mode: waveform,
            update_mode: mode,
            update_marker: 1,
            temperature: TEMP_USE_AMBIENT,
            flags: 0,
            dither_mode: 0,
            quant_bit: 0,
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
            hist_bw_waveform_mode: 0,
            hist_gray_waveform_mode: 0,
        };
        // SAFETY: `update` outlives the ioctl and matches the kernel's struct.
        unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                MXCFB_SEND_UPDATE_REX as _,
                &update as *const _,
            ) != -1
        }
    }

    /// Full-screen GC16 refresh
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

    /// Block until the EPDC has applied the last update (marker 1).
    ///
    /// Used before suspending to RAM so the panel doesn't latch mid-refresh.
    /// Best-effort: a failing ioctl is ignored, since this is purely defensive.
    pub(crate) fn wait_for_update_complete(&self) {
        let mut marker = UpdateMarkerData {
            update_marker: 1,
            collision_test: 0,
        };
        unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                MXCFB_WAIT_FOR_UPDATE_COMPLETE as _,
                &mut marker as *mut _,
            );
        }
    }

    /// Partial refresh of a dirty rectangle
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
