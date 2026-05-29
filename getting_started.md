# Getting started with Slint on Kindle

These are some pointers as to how you can set up your development environment for a smoth development experience. These are only meant as suggestions, not gospel.

## Jailbreak your Kindle

In order to run custom software on a Kindle, you need to jailbreak it first. This is not a guide on how to do that, there are plenty of great resources on the topic. Personally, I followed the guides on [https://kindlemodding.org/](https://kindlemodding.org/). You may want to install KUAL as a minimum.

## Get SSH access

After you have a jailbroken Kindle, you can install USBNetwork or USBNetLite depending on your firmware version. The guides I found for this were not too complete or coherent, but by following [these steps](https://mip-wiki.pages.dev/database/usbnet/) you should get it working. This enables you to access your kindle over SSH, either via Wifi or USB cable.

### SSH niceties

The Kindle gadget driver advertises a fixed MAC (`ee:49:00:00:00:00`) and USBNetwork brings the device up on `192.168.15.244`, with your host expected at `192.168.15.201`. Logging in as `root@192.168.15.244` every time gets old fast, so add an entry to your `~/.ssh/config`:

```
Host kindle
    HostName 192.168.15.244
    User root
```

Then `ssh kindle` is enough. Set up a key with `ssh-copy-id kindle` so you stop typing the root password, and on macOS the USB-ethernet interface keeps its config across replugs as long as the interface name sticks — so once it's configured you can deploy repeatedly without re-running `ifconfig`.


## Deploy script

`scripts/deploy-mac.sh <example-dir>` is the one-shot build-and-push for macOS over USBNetwork. Run e.g. `scripts/deploy-mac.sh clock`. It:

1. Reads the cargo package name out of `examples/<dir>/Cargo.toml` (the directory name and package name don't match, e.g. `examples/simple` → `slint-kindle-example`).
2. Finds the host interface by matching the Kindle's advertised MAC, and configures it with `192.168.15.201` if it isn't already on that subnet (this is the only step that needs `sudo`).
3. Cross-compiles for `armv7-unknown-linux-musleabihf` with `cargo zigbuild`.
4. `scp`s the binary, the launcher (as `launch-slint.sh`), and the KUAL `menu.json` to `/mnt/us`.

It `scp`s over the USB-ethernet gadget rather than mounting mass storage, so the USB port isn't cycled on every deploy. Prerequisites: `rustup target add armv7-unknown-linux-musleabihf`, `cargo install cargo-zigbuild`, a `zig` install, and USBNetwork enabled on the Kindle. Afterwards, launch from KUAL or run `launch-slint.sh <package>` over SSH.


## KUAL launcher

KUAL reads a `menu.json` from each extension directory and renders it as a menu on the device. This repo keeps the canonical copy in `kual/menu.json`; it defines a top-level **Slint Kindle** entry with one item per example, each invoking `/mnt/us/launch-slint.sh <binary-name>`:

```json
{
    "name": "Start Clock",
    "priority": 2,
    "action": "/mnt/us/launch-slint.sh slint-kindle-clock",
    "status": false,
    "internal": "status Starting Slint Kindle Clock..."
}
```

The launcher (`scripts/launch-kindle.sh`, deployed as `launch-slint.sh`) suspends the competing UI before handing the framebuffer to your app: it stops KOReader if running, disables the pillow status bar, and `SIGSTOP`s the window manager (`awesome`), content viewer (`cvm`), and main app (`KPPMainApp`). When your binary exits it `SIGCONT`s them and re-enables the pillow, so the device returns to its normal UI. App output (stdout/stderr) is captured to `/mnt/us/<binary-name>.log` for debugging. Picking an item from KUAL and running `launch-slint.sh <binary>` over SSH are equivalent — both call the same script.