slint::slint! {
    export component AppWindow inherits Window {
        Text {
            text: "Hello from Slint on Kindle";
        }
    }
}

static FONT: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

fn main() {
    slint_backend_kindle::install(FONT).expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");
    app.run().expect("event loop error");
}