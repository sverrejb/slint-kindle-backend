//! Lets background threads wake up the event loop on the UI thread.
//!
//! A pipe is shared between the loop (which waits on the read end) and any
//! background threads (which write a byte to the write end to nudge the loop).
//! Writing wakes the loop so queued closures can run on the UI thread.
//!
//! A pipe is used instead of `eventfd` so the crate still builds on macOS,
//! where `libc::eventfd` isn't available.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use slint::EventLoopError;
use slint::platform::EventLoopProxy;

pub(crate) type Closure = Box<dyn FnOnce() + Send>;
pub(crate) type Queue = Arc<Mutex<Vec<Closure>>>;

pub(crate) struct Wakeup {
    /// the event loop waits on this.
    pub(crate) read: Arc<OwnedFd>,
    /// proxies write a byte here to wake the loop.
    pub(crate) write: Arc<OwnedFd>,
}

pub(crate) struct KindleEventLoopProxy {
    pub(crate) queue: Queue,
    pub(crate) write_fd: Arc<OwnedFd>,
    pub(crate) quit_flag: Arc<AtomicBool>,
}

impl EventLoopProxy for KindleEventLoopProxy {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        self.quit_flag.store(true, Ordering::SeqCst);
        poke(&self.write_fd);
        Ok(())
    }

    fn invoke_from_event_loop(&self, event: Closure) -> Result<(), EventLoopError> {
        // Don't queue if the loop is shutting down, the closure would never
        // run, and any caller blocking on a channel send inside it would hang.
        if self.quit_flag.load(Ordering::SeqCst) {
            return Err(EventLoopError::EventLoopTerminated);
        }
        self.queue
            .lock()
            .expect("event loop closure queue poisoned")
            .push(event);
        poke(&self.write_fd);
        Ok(())
    }
}

pub(crate) fn make_wakeup() -> std::io::Result<Wakeup> {
    let mut fds: [libc::c_int; 2] = [0; 2];
    // SAFETY: pipe() fills two fds or returns -1; we check for failure.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: pipe() gave us two new fds; wrap them now so they get closed
    // automatically if anything below fails.
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };

    set_nonblock_cloexec(&read)?;
    set_nonblock_cloexec(&write)?;

    Ok(Wakeup {
        read: Arc::new(read),
        write: Arc::new(write),
    })
}

fn set_nonblock_cloexec(fd: &OwnedFd) -> std::io::Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: these fcntl variants take an int arg or none, and fd is valid for the borrow.
    unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFL);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let flags = libc::fcntl(raw, libc::F_GETFD);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::fcntl(raw, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Empty the pipe. Safe to call when there's nothing to read
pub(crate) fn drain(fd: &OwnedFd) {
    let mut buf = [0u8; 64];
    loop {
        // SAFETY: buf is valid for `buf.len()` bytes; fd is valid for the borrow.
        let n = unsafe {
            libc::read(
                fd.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n < buf.len() as isize {
            break;
        }
    }
}

fn poke(fd: &OwnedFd) {
    let byte: u8 = 1;
    // SAFETY: writing one byte to a pipe is always valid, and fd is valid for the borrow.
    unsafe {
        libc::write(
            fd.as_raw_fd(),
            &byte as *const u8 as *const libc::c_void,
            1,
        );
    }
}
