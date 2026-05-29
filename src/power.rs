//! Sysfs-driven RTC wakeup and suspend-to-RAM.
//!
//! These are the three pieces needed to implement [`crate::WakeSchedule`]:
//!
//! 1. [`find_wakealarm`] — locate the writable `/sys/class/rtc/rtcN/wakealarm`
//!    node at startup. The right `N` differs across Kindle models.
//! 2. [`arm_wakealarm`] — schedule the next wake.
//! 3. [`suspend_to_mem`] — write `mem` to `/sys/power/state`. Blocks the
//!    calling thread until the kernel resumes.
//!
//! Powerd is not stopped — the framework keeps running, we just preempt
//! powerd's own (much longer) idle-to-suspend timer.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Locate the first RTC node whose `wakealarm` file we can write.
///
/// The kernel exposes one or more `/sys/class/rtc/rtcN` directories. The one
/// that actually wires up to the SoC's wake-from-suspend IRQ varies by Kindle
/// model (typically `rtc0` on PW2+, `rtc1` on older devices). We try them in
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
///
/// Writes `0` first to clear any existing alarm — without that, the kernel
/// returns `EBUSY` if an alarm is already armed (powerd may have set one).
///
/// We use the relative `+N` form rather than an absolute epoch second so the
/// schedule is anchored to the RTC's own clock. On a Kindle without internet
/// sync, `SystemTime::now()` can drift from the RTC, which would make an
/// absolute alarm fire earlier or later than the consumer asked for.
pub(crate) fn arm_wakealarm(wakealarm: &Path, delay: Duration) -> std::io::Result<()> {
    std::fs::write(wakealarm, b"0\n")?;

    let seconds = delay.as_secs().max(1);
    std::fs::write(wakealarm, format!("+{seconds}").as_bytes())
}

/// Write `mem` to `/sys/power/state` and block until the kernel resumes.
///
/// The whole process (and the rest of userspace) is frozen for the duration
/// of the suspend. On return, we are awake again.
pub(crate) fn suspend_to_mem() -> std::io::Result<()> {
    std::fs::write("/sys/power/state", b"mem")
}
