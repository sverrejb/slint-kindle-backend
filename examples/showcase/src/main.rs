// AI disclosure: This example was in large generated using CLaude.

slint::include_modules!();

#[cfg(all(target_arch = "arm", target_os = "linux", target_env = "musl"))]
static SANS_REGULAR: &[u8] = include_bytes!("../../LiberationSans-Regular.ttf");

#[cfg(all(target_arch = "arm", target_os = "linux", target_env = "musl"))]
fn install_backend() -> &'static str {
    let backend =
        slint_backend_kindle::install(SANS_REGULAR).expect("failed to install Kindle backend");
    backend.set_black_and_white(true);
    "Kindle E-ink"
}

#[cfg(not(all(target_arch = "arm", target_os = "linux", target_env = "musl")))]
fn install_backend() -> &'static str {
    "desktop preview"
}

fn main() {
    let platform = install_backend();

    let app = AppWindow::new().expect("failed to create window");

    #[cfg(not(all(target_arch = "arm", target_os = "linux", target_env = "musl")))]
    app.window().set_size(slint::PhysicalSize::new(600, 800));

    app.set_platform_name(platform.into());
    app.on_quit(|| std::process::exit(0));

    app.run().expect("event loop error");
}
