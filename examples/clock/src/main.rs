use std::time::Duration;

use slint::{Timer, TimerMode};

slint::include_modules!();

static LIBERATION_SANS: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

fn main() {
    slint_backend_kindle::install(LIBERATION_SANS)
        .expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");
    app.on_quit(|| std::process::exit(0));

    let tick = {
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let now = chrono::Local::now();
            app.set_time_text(now.format("%H:%M").to_string().into());
        }
    };
    tick();

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), tick);

    app.run().expect("event loop error");
}
