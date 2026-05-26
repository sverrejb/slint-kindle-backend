use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::LogicalPosition;

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

pub(crate) struct TouchInput {
    fd: libc::c_int,
    tracking_id: i32,
    x: f32,
    y: f32,
    pressed: bool,
}

impl TouchInput {
    pub(crate) fn open() -> std::io::Result<Self> {
        let path = std::ffi::CString::new("/dev/input/event1").unwrap();
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { fd, tracking_id: -1, x: 0.0, y: 0.0, pressed: false })
    }

    /// Read any waiting touch events and forward them to the window as pointer events.
    pub(crate) fn poll(&mut self, window: &MinimalSoftwareWindow) {
        while let Some(event) = self.read_event() {
            match (event.kind, event.code) {
                (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X) => self.x = event.value as f32,
                (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y) => self.y = event.value as f32,
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
            WindowEvent::PointerPressed { position, button: PointerEventButton::Left }
        };
        let _ = window.try_dispatch_event(pointer_event);
    }
}

impl Drop for TouchInput {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}
