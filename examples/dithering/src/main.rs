// Dithering comparison sheet. Each row is one technique; each row is split into
// shade bands running light -> dark. Every band is rendered as pure black/white
// pixels (except the "True grey" reference), so the dithered rows update
// flash-free on E-ink while the grey reference flashes — the whole motivation
// for dithering on this hardware.
//
// Cross-platform like examples/cross-platform: the same source runs on the
// Kindle's E-ink backend and in a desktop preview window. The dithering output
// is identical on both — it's plain pixel generation — so you can eyeball the
// patterns on your desktop, then deploy to judge them on the actual panel.

mod dither;

use dither::{Ditherer, Method, ROWS};
use slint::{Image, ModelRc, Rgb8Pixel, SharedPixelBuffer, SharedString, VecModel};

slint::include_modules!();

// Only the Kindle needs a bundled font (no system fonts on the device). The
// desktop preview uses the OS's fonts via Slint's default backend.
#[cfg(all(target_arch = "arm", target_os = "linux", target_env = "musl"))]
static LIBERATION_SANS: &[u8] = include_bytes!("../../LiberationSans-Regular.ttf");

#[cfg(all(target_arch = "arm", target_os = "linux", target_env = "musl"))]
fn install_backend() {
    // Leave black-and-white mode OFF: the dithered rows are already pure B/W,
    // and keeping grey live lets the "True grey" reference row flash so the
    // contrast with the dithered rows is visible on the panel.
    slint_backend_kindle::install(LIBERATION_SANS).expect("failed to install Kindle backend");
}

#[cfg(not(all(target_arch = "arm", target_os = "linux", target_env = "musl")))]
fn install_backend() {}

// Logical px == physical px on the Kindle (scale factor 1.0), so these are also
// the exact pixel dimensions of the generated swatch images. Keeping the Slint
// element sizes equal to the source image sizes means a 1:1 blit with no
// resampling, which is essential — interpolating a dither pattern destroys it.
const LABEL_W: f32 = 180.0;
const BAND_W: u32 = 70;
const ROW_H: u32 = 78;

/// Grey shades for the bands, light -> dark. Interior steps only (no pure white
/// or black), evenly spaced across the tonal range so each technique is judged
/// on the midtones it actually has to fake.
fn shades() -> Vec<u8> {
    const N: usize = 12;
    (0..N)
        .map(|i| {
            let frac = (i + 1) as f32 / (N + 1) as f32; // ink coverage 1/13 .. 12/13
            (255.0 * (1.0 - frac)).round() as u8
        })
        .collect()
}

/// Render one technique's full row: a strip image of `shades.len()` bands.
fn build_strip(ditherer: &Ditherer, method: Method, shades: &[u8]) -> Image {
    let width = BAND_W * shades.len() as u32;
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(width, ROW_H);
    let pixels = buffer.make_mut_slice();
    let stride = width as usize;
    for (i, &shade) in shades.iter().enumerate() {
        let band = dither::Band {
            x0: i * BAND_W as usize,
            stride,
            w: BAND_W as usize,
            h: ROW_H as usize,
        };
        ditherer.render_band(method, shade, pixels, &band);
    }
    Image::from_rgb8(buffer)
}

fn main() {
    install_backend();

    let app = AppWindow::new().expect("failed to create window");
    app.on_quit(|| std::process::exit(0));

    let ditherer = Ditherer::new();
    let shades = shades();

    let rows: Vec<DitherRow> = ROWS
        .iter()
        .map(|&(name, method)| DitherRow {
            name: SharedString::from(name),
            strip: build_strip(&ditherer, method, &shades),
        })
        .collect();

    // Column headers: ink coverage as a percentage (0% = white, 100% = black).
    let labels: Vec<SharedString> = shades
        .iter()
        .map(|&s| SharedString::from(format!("{}%", ((255 - s as u32) * 100 + 127) / 255)))
        .collect();

    app.set_label_w(LABEL_W);
    app.set_band_w(BAND_W as f32);
    app.set_row_h(ROW_H as f32);
    app.set_shade_labels(ModelRc::new(VecModel::from(labels)));
    app.set_rows(ModelRc::new(VecModel::from(rows)));

    app.run().expect("event loop error");
}
