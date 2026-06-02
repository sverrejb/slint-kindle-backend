//! Sysfs-driven RTC wakeup and suspend-to-RAM.
//!
//! Powerd is not stopped — the framework keeps running, we just preempt
//! powerd's own (probably) longer idle-to-suspend timer.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Locate the first RTC node whose `wakealarm` file we can write. We try them in
/// order and pick the first that exists with a writable `wakealarm`.
pub(crate) fn find_wakealarm() -> std::io::Result<PathBuf> {
    for n in 0..4 {
        let candidate = PathBuf::from(format!("/sys/class/rtc/rtc{n}/wakealarm"));
        match std::fs::OpenOptions::new().write(true).open(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(_) => continue,
        }
    }
    Err(std::io::Error::new(
        ErrorKind::NotFound,
        "no writable /sys/class/rtc/rtcN/wakealarm found",
    ))
}

/// Arm the RTC to fire `delay` from now.
/// Writes `0` first to clear any existing alarm — without that, the kernel
/// returns `EBUSY` if an alarm is already armed (powerd may have set one).
pub(crate) fn arm_wakealarm(wakealarm: &Path, delay: Duration) -> std::io::Result<()> {
    std::fs::write(wakealarm, b"0\n")?;

    let seconds = delay.as_secs().max(1);
    std::fs::write(wakealarm, format!("+{seconds}").as_bytes())
}

/// Write `mem` to `/sys/power/state` and block until the kernel resumes.
/// The whole process (and the rest of userspace) is frozen for the duration
/// of the suspend. On return, we are awake again.
pub(crate) fn suspend_to_mem() -> std::io::Result<()> {
    std::fs::write("/sys/power/state", b"mem")
}
