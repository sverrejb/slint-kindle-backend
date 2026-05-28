use std::process::Command;

slint::include_modules!();

static LIBERATION_SANS: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

fn main() {
    slint_backend_kindle::install(LIBERATION_SANS).expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");

    if let Some(current) = read_frontlight() {
        app.set_level(current);
    }

    app.on_quit(|| std::process::exit(0));
    app.on_level_changed(set_frontlight);

    app.run().expect("event loop error");
}

// lipc is Lab126's IPC bus. flIntensity exposes the frontlight. lipc-set-prop is preferred over poking sysfs directly because
// the powerd path differs across Kindle models.
fn set_frontlight(level: i32) {
    let _ = Command::new("lipc-set-prop")
        .args(["com.lab126.powerd", "flIntensity", &level.to_string()])
        .status();
}

fn read_frontlight() -> Option<i32> {
    let out = Command::new("lipc-get-prop")
        .args(["com.lab126.powerd", "flIntensity"])
        .output()
        .ok()?;
    std::str::from_utf8(&out.stdout).ok()?.trim().parse().ok()
}
