# Slint backend for Kindle

Slint backend for jailbroken Kindles. Allows for running Slint GUIS on Kindle devices.

> ⚠️ **Experimental crate: limited device support.**
> This crate is experimental and has not been tested on a wide variety of Kindle devices. See "Tested devices" further down. Please file an issue (or a PR) if you try it on different hardware!

<img src="https://raw.githubusercontent.com/sverrejb/slint-kindle-backend/main/demo.webp" alt="Slint app running on a Kindle Paperwhite" width="750">

## Usage

For suggestions on how to set up your dev environment, see the [getting started doc](https://github.com/sverrejb/slint-kindle-backend/blob/main/getting_started.md).

Add the crate to your app:

```sh
cargo add slint --no-default-features --features compat-1-2,std,renderer-software
cargo add slint-backend-kindle
```

Slint is added with `--no-default-features` and only `compat-1-2`, `std`, and `renderer-software` because the Kindle has no GPU — any hardware-renderer feature is meaningless and would pull in unwanted system dependencies. `renderer-software` specifically is required: it's the only renderer that can drive the Kindle framebuffer.

Bundle a TTF/OTF font with your app and pass it to `install()` at startup. **The font is required**. The various Kindle models has no fontconfig and no default location for system fonts, so Slint's software renderer would panic on the first fallback query without one.

```rust
slint::include_modules!();

static FONT: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

fn main() {
    slint_backend_kindle::install(FONT).expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");
    app.run().expect("event loop error");
}
```

The font becomes the default, so Slint widgets that don't specify `font-family` render correctly. You can still reference the font by its real family name in your `.slint` files (e.g. `font-family: "Liberation Sans"`).

### Additional fonts

`install()` returns a `KindleBackend` handle. To use more than one typeface, register the extras on the handle **after** constructing the window:

```rust
static DEFAULT_FONT: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");
static FANCY_FONT: &[u8] = include_bytes!("../fonts/DancingScript-Regular.ttf");

fn main() {
    let backend = slint_backend_kindle::install(DEFAULT_FONT)
        .expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");
    backend.register_font_from_memory(FANCY_FONT)
        .expect("failed to register font");
    app.run().expect("event loop error");
}
```

Reference each font in `.slint` by its **real family name** (the one in the font's `name` table), not the filename. `DancingScript-Regular.ttf` for instance reports itself as `"Dancing Script"`, so the .slint must say `font-family: "Dancing Script"`. If a glyph fails to render, that mismatch is the first thing to check — `fc-query font.ttf` or `otfinfo --info font.ttf` will show the family string the font advertises.

## Cross-compiling for the Kindle

The Kindle runs an ARMv7 musl userland. Recommended toolchain:

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo install cargo-zigbuild
# brew install zig    # or your platform's equivalent

cargo zigbuild --release --target armv7-unknown-linux-musleabihf
```

The resulting binary is statically linked against musl and runs directly on the device.

## Tested devices
So far, the backend has been tested to work on:
* Kindle Paperwhite 7th gen.
* Kindle Touch 4th gen
 

## Roadmap
* Examples
* Better device support
* Font discovery instead of hard coded default

## License

The code in this crate is dual-licensed under either of

* MIT License ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/license/mit)
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or https://www.apache.org/licenses/LICENSE-2.0)

at your option.

Note that this crate depends on [`slint`](https://crates.io/crates/slint), which is licensed under `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`. **Any application built using this backend and Slint must comply with one of Slint's licenses.**