slint::include_modules!();

static LIBERATION_SANS: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

fn main() {
    slint_backend_kindle::install(LIBERATION_SANS)
        .expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");
    app.on_quit(|| std::process::exit(0));
    app.run().expect("event loop error");
}
