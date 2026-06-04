//! Kernel ABI: framebuffer + Kindle EPDC structs and ioctl numbers.

// Standard Linux framebuffer ioctl numbers (see <linux/fb.h>).
// Typed as c_ulong (not libc::Ioctl) so the crate still type-checks on
// non-Linux dev hosts where libc::Ioctl isn't defined, like macos
pub(super) const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
pub(super) const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

// These structs mirror the kernel's `fb_var_screeninfo` and `fb_fix_screeninfo`.
// We only read from them, fields we care about are `xres`, `yres` (visible
// resolution) and `line_length` (stride in bytes per row, which may be larger
// than xres due to alignment padding).

#[repr(C)]
#[derive(Default)]
pub(super) struct FbBitfield {
    pub(super) offset: u32,
    pub(super) length: u32,
    pub(super) msb_right: u32,
}

#[repr(C)]
#[derive(Default)]
pub(super) struct FbVarScreeninfo {
    pub(super) xres: u32,
    pub(super) yres: u32,
    pub(super) xres_virtual: u32,
    pub(super) yres_virtual: u32,
    pub(super) xoffset: u32,
    pub(super) yoffset: u32,
    pub(super) bits_per_pixel: u32,
    pub(super) grayscale: u32,
    pub(super) red: FbBitfield,
    pub(super) green: FbBitfield,
    pub(super) blue: FbBitfield,
    pub(super) transp: FbBitfield,
    pub(super) nonstd: u32,
    pub(super) activate: u32,
    pub(super) height: u32,
    pub(super) width: u32,
    pub(super) accel_flags: u32,
    pub(super) pixclock: u32,
    pub(super) left_margin: u32,
    pub(super) right_margin: u32,
    pub(super) upper_margin: u32,
    pub(super) lower_margin: u32,
    pub(super) hsync_len: u32,
    pub(super) vsync_len: u32,
    pub(super) sync: u32,
    pub(super) vmode: u32,
    pub(super) rotate: u32,
    pub(super) colorspace: u32,
    pub(super) reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default)]
pub(super) struct FbFixScreeninfo {
    pub(super) id: [u8; 16],
    pub(super) smem_start: libc::c_ulong,
    pub(super) smem_len: u32,
    pub(super) type_: u32,
    pub(super) type_aux: u32,
    pub(super) visual: u32,
    pub(super) xpanstep: u16,
    pub(super) ypanstep: u16,
    pub(super) ywrapstep: u16,
    pub(super) line_length: u32,
    pub(super) mmio_start: libc::c_ulong,
    pub(super) mmio_len: u32,
    pub(super) accel: u32,
    pub(super) capabilities: u16,
    pub(super) reserved: [u16; 2],
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
pub(super) struct AlternateBuffer {
    pub(super) physical_address: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) update_region: UpdateRect,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct UpdateRequest {
    pub(super) update_region: UpdateRect,
    pub(super) waveform_mode: u32,
    pub(super) update_mode: u32,
    pub(super) update_marker: u32,
    pub(super) previous_bw_waveform_mode: u32,
    pub(super) previous_gray_waveform_mode: u32,
    pub(super) temperature: i32,
    pub(super) flags: u32,
    pub(super) alternate_buffer: AlternateBuffer,
}

#[repr(C)]
pub(super) struct UpdateRequestRex {
    pub(super) update_region: UpdateRect,
    pub(super) waveform_mode: u32,
    pub(super) update_mode: u32,
    pub(super) update_marker: u32,
    pub(super) temperature: i32,
    pub(super) flags: u32,
    pub(super) dither_mode: i32,
    pub(super) quant_bit: i32,
    pub(super) alternate_buffer: AlternateBuffer,
    pub(super) hist_bw_waveform_mode: u32,
    pub(super) hist_gray_waveform_mode: u32,
}

// Kindle EPDC ioctl and constants.
// The ioctl number was confirmed by stracing `eips` on a real device.
pub(super) const MXCFB_SEND_UPDATE: libc::c_ulong = 0x4048_462e;

// ioctl number for Kindle Paperwhite 10th gen, from strace.
pub(super) const MXCFB_SEND_UPDATE_REX: libc::c_ulong = 0x4050_462e;

// Blocks until the EPDC has finished applying the update with a given marker.
pub(super) const MXCFB_WAIT_FOR_UPDATE_COMPLETE: libc::c_ulong = 0xc008_462f;

#[repr(C)]
#[derive(Default)]
pub(super) struct UpdateMarkerData {
    pub(super) update_marker: u32,
    pub(super) collision_test: u32,
}

pub(super) const WAVEFORM_MODE_GC16: u32 = 2; // Full 16-level grayscale refresh (slow, high quality)
pub(super) const WAVEFORM_MODE_AUTO: u32 = 257; // Let the driver pick the best waveform
pub(super) const UPDATE_MODE_PARTIAL: u32 = 0; // Only redraw the dirty region
pub(super) const UPDATE_MODE_FULL: u32 = 1; // Flash the whole screen (clears ghosting)
pub(super) const TEMP_USE_AMBIENT: i32 = 0x1000; // Use the panel's ambient temperature sensor
