# Slint backend for Kindle

Slint backend for jailbroken Kindles. Allows for running Slint GUIS on Kindle devices.

> ⚠️ **Experimental crate: limited device support.**
> This crate is experimental and has not been tested on a wide variety of Kindle devices. See "Tested devices" further down. Please file an issue (or a PR) if you try it on different hardware!

<img src="https://raw.githubusercontent.com/sverrejb/slint-kindle-backend/main/demo.webp" alt="Slint app running on a Kindle Paperwhite" width="750">


## Features

* **Custom fonts**: support for configuring a default font + additional ones.
* **Idle event loop**: blocks in `poll(2)` when there's nothing to do, so the SoC can idle instead of burning cpu cycles.
* **Suspend-and-wake cycle**: lets the device sleep between periodic display updates. Useful for long battery life applications.
* **E-ink rendering via the EPDC driver**: No dependency on X11 etc.
* **Pure black & white (bilevel) mode**: optional flicker-free rendering, so flicker less. Great for high-interaction UIs.

## Usage and configuration

For suggestions on how to set up your dev environment, see the [getting started doc](https://github.com/sverrejb/slint-kindle-backend/blob/main/getting_started.md).

Add the Slint crate and the backend to your app:

```sh
cargo add slint --no-default-features --features compat-1-2,std,renderer-software
cargo add slint-backend-kindle
```

Slint is added with `--no-default-features` and only `compat-1-2`, `std`, and `renderer-software` because the Kindle has no GPU — any hardware-renderer feature is meaningless and would pull in unwanted system dependencies. `renderer-software` specifically is required: it's the only renderer that can drive the Kindle framebuffer.

Bundle a TTF/OTF font with your app and pass it to `install()` at startup. **The font is required**. The various Kindle models has no fontconfig and no default location for system fonts, so Slint's software renderer would panic on the first fallback query without one.

```rust
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
```

The UI is declared inline with the `slint::slint!` macro, so this example needs no `.slint` file or `build.rs`. For a "real" application you'd typically keep the markup in a `.slint` file and pull it in with `slint::include_modules!()` instead. See the [Slint documentation](https://docs.slint.dev/latest/docs/slint/) for how to build with Slint.

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


### Long battery life: Wake from suspend with a schedule.

By default the event loop blocks in `poll(2)` when idle, so the system idles but doesn't enter the deep suspend-to-RAM state that `powerd` would normally use. For prolonged stand-by applications that you want to still update the display periodically, opt in to a wake schedule so the device actually sleeps between updates and wakes on its own to refresh:

```rust
use std::time::Duration;
use slint_backend_kindle::WakeSchedule;

fn main() {
    let backend = slint_backend_kindle::install(FONT)
        .expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");

    // After 30s of no touch, suspend to RAM. Wake every 5 minutes to refresh.
    let backend = backend.set_wake_schedule(WakeSchedule {
        wake_interval: Duration::from_secs(5 * 60),
        stay_awake: Duration::from_secs(30),
    });

    // Optional: run something each time the device wakes, like polling an API to update your view or whatever.
    backend.on_wake(|| {
        refresh_data();
    });

    app.run().expect("event loop error");
}
```

Touch activity during the awake window resets `stay_awake`, exactly like the device's normal idle timer. The cycle suppresses itself while Slint animations or queued event-loop closures are pending, so it never interrupts active UI work.

`set_wake_schedule` consumes the backend and returns a `KindleBackend<Scheduled>`. `on_wake` is only available on that scheduled form. Call `set_wake_schedule` again on the scheduled backend to change the schedule at runtime, or `clear_wake_schedule()` to disable suspension entirely.

#### Tips for `on_wake()`

- The callback runs **synchronously on the UI thread** before the next render. For HTTP calls etc you probably want to spawn a background thread and marshal results back via `slint::invoke_from_event_loop`.
- **Wifi reconnects ~3–10 s after each resume.** Don't expect networking to work on the first attempt. Retry with backoff inside your callback, or add a delay before doing anything network-related.
- **Slint timers still fire on resume** (any timer whose deadline elapsed during sleep ticks once), but they don't align with `wake_interval`. If you specifically need work done at each wake, put it in `on_wake`, don't rely on a `Timer::Repeated` matching the schedule.

#### Notes when developing and testing 

- **Connecting via USB cable seems blocks suspend.** When the device plugged in via USBNetwork (or just plugged in as USBMS) it seemed for me to not go into deep suspend.  Deploy the binary, then physically unplug if you are testing if suspend-to-ram is working properly.

### Pure black and white (bilevel): flicker-free updates

An E-ink panel can flip a pixel between pure black and white quickly and effortlessly, but showing any **grey** might **<sup>*</sup>** need a waveform that briefly drives the pixels *through black* before settling, causing a flicker. So grey fills, fading animations etc flashes on every update, while pure black-and-white content updates more cleanly.

Pure black-and-white mode is available for applications where you want as little flickering as poissible. It thresholds every pixel to pure black or white (at the luma midpoint) before it reaches the framebuffer, so the panel only ever does its fast, flicker-free update:

```rust
fn main() {
    let backend = slint_backend_kindle::install(FONT)
        .expect("failed to install Kindle backend");
    backend.set_black_and_white(true);   // pure black/white, flash-free

    let app = AppWindow::new().expect("failed to create window");
    app.run().expect("event loop error");
}
```

Buttons, toggles, drawing, and anything that redraws on touch update instantly with no black flash, much closer to "instant" than a greyscale UI feels on E-ink.

The trade-off is no anti-aliasing: text edges and thin strokes get harder/blockier, and any light grey is pushed to white (so it disappears). If you want to use this mode you should design for it: Use solid black and white.

<sup>*</sup>That is, at least on the author's device. This might vary between models.

## Cross-compiling for the Kindle

The Kindle runs an ARMv7 musl userland. Suggested toolchain:

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo install cargo-zigbuild
brew install zig    # or your platform's equivalent

cargo zigbuild --release --target armv7-unknown-linux-musleabihf
```

The resulting binary is statically linked against musl and runs directly on the device.

## Tested devices
So far, the backend has been tested to work on:
* Kindle Paperwhite 7th gen (PW3).
* Kindle Paperwhite 10th gen (PW4) - Thanks, [gmemstr](https://github.com/gmemstr)!
* Kindle Touch 4th gen - Thanks, [cmeister2](https://github.com/cmeister2)!
 

## Roadmap
* More and better examples
* Better device support / testing of device support
* Font discovery instead of hard coded default
* Optional `wait_for_link_up()` helper for `on_wake` callbacks that need wifi
* User-facing refresh display functions for manual clearing of ghosting etc.

## License

The code in this crate is dual-licensed under either of

* MIT License ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/license/mit)
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or https://www.apache.org/licenses/LICENSE-2.0)

at your option.

Note that this crate depends on [`slint`](https://crates.io/crates/slint), which is licensed under `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`. **Any application built using this backend and Slint must comply with one of Slint's licenses.**