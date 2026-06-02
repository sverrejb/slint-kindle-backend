// The same UI and logic build for the Kindle and
// for your dev machine. Only the platform setup differs, selected at compile
// time. Develop on your desktop OS, then cross-compile the same binary source for the device.

slint::include_modules!();

// This is only needed on the Kindle
#[cfg(all(target_arch = "arm", target_os = "linux", target_env = "musl"))]
static LIBERATION_SANS: &[u8] = include_bytes!("../../LiberationSans-Regular.ttf");

// Kindle only: install the E-ink platform backend before creating any window.
#[cfg(all(target_arch = "arm", target_os = "linux", target_env = "musl"))]
fn install_backend() -> &'static str {
    slint_backend_kindle::install(LIBERATION_SANS).expect("failed to install Kindle backend");
    "Kindle E-ink"
}

// Non-kindle: do nothing and let Slint pick its default desktop backend.
#[cfg(not(all(target_arch = "arm", target_os = "linux", target_env = "musl")))]
fn install_backend() -> &'static str {
    "desktop preview"
}

fn main() {
    let platform = install_backend();

    let app = AppWindow::new().expect("failed to create window");
    app.set_platform_name(platform.into());
    app.on_quit(|| std::process::exit(0));

    app.run().expect("event loop error");
}
