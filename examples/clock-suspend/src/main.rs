use std::time::Duration;

use slint::{Timer, TimerMode};
use slint_backend_kindle::WakeSchedule;

slint::include_modules!();

static LIBERATION_SANS: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

fn main() {
    let backend =
        slint_backend_kindle::install(LIBERATION_SANS).expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");
    app.on_quit(|| std::process::exit(0));

    let tick = {
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let now = chrono::Local::now();
            app.set_time_text(now.format("%H:%M:%S").to_string().into());
        }
    };
    tick();

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), tick);

    // After 10s of no touch the device suspends to RAM, wakes 1 min later,
    // redraws the clock, then suspends again. Touching the screen during the
    // awake window resets the 10s countdown, like the device's normal idle
    // timer.
    let backend = backend.set_wake_schedule(WakeSchedule {
        wake_interval: Duration::from_secs(60),
        stay_awake: Duration::from_secs(10),
    });

    // Refresh the displayed time the instant we resume, before the next
    // render — without this, the screen would show the pre-suspend time
    // until the 1 Hz timer's next tick.
    backend.on_wake({
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let now = chrono::Local::now();
            app.set_time_text(now.format("%H:%M:%S").to_string().into());
        }
    });

    app.run().expect("event loop error");
}
