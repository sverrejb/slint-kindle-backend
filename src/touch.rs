use slint::LogicalPosition;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{PointerEventButton, WindowEvent};

// One event from the touchscreen driver. Layout has to match exactly what the kernel writes.
#[repr(C)]
struct InputEvent {
    timestamp_seconds: u32,
    timestamp_microseconds: u32,
    kind: u16,
    code: u16,
    value: i32,
}

// Kernel-defined IDs we match against the `kind` and `code` fields of each event.
const EVENT_SYNC: u16 = 0;
const EVENT_ABSOLUTE_AXIS: u16 = 3;
const SYNC_REPORT: u16 = 0;
const TOUCH_POSITION_X: u16 = 0x35;
const TOUCH_POSITION_Y: u16 = 0x36;
const TOUCH_TRACKING_ID: u16 = 0x39;

// The following constants/functions replicate Linux kernel C macros from <linux/input.h>
// that aren't exposed by the `libc` crate. The formulas expand the _IOW/_IOR macro:
//   _IOC(dir, type, nr, size) = (dir << 30) | (size << 16) | (type << 8) | nr

// EVIOCGRAB: exclusively grabs an input device so no other process receives
// its events. This is essential on Kindle Touch because the zforce touch controller
// is initialized by the framework/X — if we stop the framework, zforce disconnects
// and becomes unusable. Instead, we keep the framework running and grab the device.
// C macro: #define EVIOCGRAB _IOW('E', 0x90, int)
const EVIOCGRAB: libc::c_ulong = 0x40044590;

// C macro: #define EVIOCGABS(abs) _IOR('E', 0x40 + (abs), struct input_absinfo)
// struct input_absinfo is 6 × i32 = 24 bytes
fn eviocgabs(axis: u8) -> libc::c_ulong {
    (2 << 30) | (24 << 16) | ((b'E' as libc::c_ulong) << 8) | (0x40 + axis as libc::c_ulong)
}

/// Kernel struct returned by EVIOCGABS.
#[repr(C)]
#[derive(Default)]
struct InputAbsinfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

pub(crate) struct TouchInput {
    fd: libc::c_int,
    tracking_id: i32,
    x: f32,
    y: f32,
    pressed: bool,
    screen_width: f32,
    screen_height: f32,
    /// Maximum raw X value reported by the touch controller (from EVIOCGABS).
    max_x: f32,
    /// Maximum raw Y value reported by the touch controller (from EVIOCGABS).
    max_y: f32,
}

impl TouchInput {
    pub(crate) fn open(screen_width: u32, screen_height: u32) -> std::io::Result<Self> {
        let path = Self::find_touch_device().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no touch input device found (looked for ABS_MT_POSITION_X in /dev/input/event*)",
            )
        })?;
        let c_path = std::ffi::CString::new(path).unwrap();
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Exclusively grab the device so X/framework doesn't consume our events.
        let ret = unsafe { libc::ioctl(fd, EVIOCGRAB as libc::Ioctl, 1 as libc::c_int) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(err);
        }

        // Query the axis ranges from the device so we scale correctly.
        let max_x = Self::query_axis_max(fd, TOUCH_POSITION_X as u8).unwrap_or(4095) as f32;
        let max_y = Self::query_axis_max(fd, TOUCH_POSITION_Y as u8).unwrap_or(4095) as f32;

        Ok(Self {
            fd,
            tracking_id: -1,
            x: 0.0,
            y: 0.0,
            pressed: false,
            screen_width: screen_width as f32,
            screen_height: screen_height as f32,
            max_x,
            max_y,
        })
    }

    /// Query the maximum value for an absolute axis via EVIOCGABS.
    fn query_axis_max(fd: libc::c_int, axis: u8) -> Option<i32> {
        let mut info = InputAbsinfo::default();
        let ret = unsafe {
            libc::ioctl(
                fd,
                eviocgabs(axis) as libc::Ioctl,
                &mut info as *mut InputAbsinfo,
            )
        };
        if ret < 0 { None } else { Some(info.maximum) }
    }

    /// Scan /dev/input/event* for a device that reports ABS_MT_POSITION_X (0x35).
    fn find_touch_device() -> Option<String> {
        for n in 0..10 {
            let path = format!("/dev/input/event{n}");
            let c_path = std::ffi::CString::new(path.as_str()).unwrap();
            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
            if fd < 0 {
                continue;
            }
            // EVIOCGBIT(EV_ABS, size) = _IOC(_IOC_READ, 'E', 0x20 + EV_ABS, size)
            // For ABS (0x03): ioctl number is 0x80404520 + 0x03 = we compute it manually.
            // We need at least 8 bytes to cover bit 0x35 (byte index 6, bit 5).
            let mut abs_bits = [0u8; 8];
            // EVIOCGBIT(EV_ABS=3, 8) on 32-bit: _IOC(2, 'E', 0x23, 8)
            let request: libc::c_ulong = 2 << 30 | 8 << 16 | (b'E' as libc::c_ulong) << 8 | 0x23;
            let ret = unsafe { libc::ioctl(fd, request as libc::Ioctl, abs_bits.as_mut_ptr()) };
            unsafe { libc::close(fd) };
            if ret < 0 {
                continue;
            }
            // Check bit 0x35 (ABS_MT_POSITION_X): byte 0x35/8 = 6, bit 0x35%8 = 5
            if abs_bits[6] & (1 << 5) != 0 {
                return Some(path);
            }
        }
        None
    }

    /// Read any waiting touch events and forward them to the window as pointer events.
    pub(crate) fn poll(&mut self, window: &MinimalSoftwareWindow) {
        while let Some(event) = self.read_event() {
            match (event.kind, event.code) {
                (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X) => {
                    self.x = (event.value as f32) * self.screen_width / self.max_x;
                }
                (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y) => {
                    self.y = (event.value as f32) * self.screen_height / self.max_y;
                }
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID) => {
                    self.tracking_id = event.value;
                    if event.value == -1 {
                        self.release(window);
                    }
                }
                (EVENT_SYNC, SYNC_REPORT) => self.commit(window),
                _ => {}
            }
        }
    }

    fn read_event(&self) -> Option<InputEvent> {
        let mut event = InputEvent {
            timestamp_seconds: 0,
            timestamp_microseconds: 0,
            kind: 0,
            code: 0,
            value: 0,
        };
        let bytes_read = unsafe {
            libc::read(
                self.fd,
                &mut event as *mut InputEvent as *mut libc::c_void,
                std::mem::size_of::<InputEvent>(),
            )
        };
        (bytes_read > 0).then_some(event)
    }

    fn release(&mut self, window: &MinimalSoftwareWindow) {
        if !self.pressed {
            return;
        }
        self.pressed = false;
        let _ = window.try_dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(self.x, self.y),
            button: PointerEventButton::Left,
        });
    }

    // Called when the driver signals "this batch of events is complete". Dispatch
    // a press if the finger just touched down, or a move if it was already down.
    fn commit(&mut self, window: &MinimalSoftwareWindow) {
        if self.tracking_id < 0 {
            return;
        }
        let position = LogicalPosition::new(self.x, self.y);
        let pointer_event = if self.pressed {
            WindowEvent::PointerMoved { position }
        } else {
            self.pressed = true;
            WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            }
        };
        let _ = window.try_dispatch_event(pointer_event);
    }
}

impl Drop for TouchInput {
    fn drop(&mut self) {
        // Release the exclusive grab before closing
        unsafe { libc::ioctl(self.fd, EVIOCGRAB as libc::Ioctl, 0 as libc::c_int) };
        unsafe { libc::close(self.fd) };
    }
}
