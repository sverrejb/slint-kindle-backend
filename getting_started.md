# Getting started with Slint on Kindle

These are some pointers as to how you can set up your development environment for a smoth development experience. These are only meant as suggestions, not gospel.

## Jailbreak your Kindle

In order to run custom software on a Kindle, you need to jailbreak it first. This is not a guide on how to do that, there are plenty of great resources on the topic. Personally, I followed the guides on [https://kindlemodding.org/](https://kindlemodding.org/). You may want to install KUAL as a minimum.

## Get SSH access

After you have a jailbroken Kindle, you can install USBNetwork or USBNetLite depending on your firmware version. The guides I found for this were not too complete or coherent, but by following [these steps](https://mip-wiki.pages.dev/database/usbnet/) you should get it working. This enables you to access your kindle over SSH, either via Wifi or USB cable.

> **Note:** When connecting over USB, you'll usually need to bring up the host-side USB-ethernet interface and give it an address on the Kindle's subnet before SSH works. On macOS this looks like:
>
> ```sh
> sudo ifconfig en9 192.168.15.201 netmask 255.255.255.0 up
> ```
>
> On Linux the equivalent is `ip`:
>
> ```sh
> sudo ip addr add 192.168.15.201/24 dev <iface>
> sudo ip link set <iface> up
> ```
>
> Replace `en9`/`<iface>` with your actual interface (check `ifconfig`/`ip link`). The deploy script further down does this `ip`-based setup automatically.

### SSH niceties

Logging in as `root@192.168.15.244` every time gets old fast, so add an entry to your `~/.ssh/config`:

```
Host kindle
    HostName 192.168.15.244
    User root
```

Then `ssh kindle` is enough. And while ssh-copy-id does not work, copying your .pub file to /mnt/us/usbnet/etc/authorized_keys (authorized_keys is the file, not a folder) should work.

## Launch script

A launch script is usefule for suspending processes that might interfere with running your app (like writing to the screen buffer etc). This script example hands the framebuffer to your app, and restores everything on exit. This is only meant as an example, and might/will need to be adjusted accoring to your needs, kindle model and other factors.

```sh
#!/bin/sh
BIN="my-app"
APP="/mnt/us/$BIN"


if pidof reader.lua >/dev/null 2>&1; then
    echo "Stopping KOReader..."
    kill $(pidof reader.lua) 2>/dev/null
    sleep 1
fi

lipc-set-prop com.lab126.pillow disableEnablePillow disable 2>/dev/null

killall -STOP awesome 2>/dev/null
killall -STOP cvm 2>/dev/null
killall -STOP KPPMainApp 2>/dev/null

usleep 300000

LOG_DIR="/mnt/us/$BIN-logs"
mkdir -p "$LOG_DIR"
RUN_TS=$(date +%Y%m%dT%H%M%S)
export RUN_TS

echo "Starting $BIN... (logs: $LOG_DIR/${RUN_TS}-*)"
"$APP" >"$LOG_DIR/${RUN_TS}-${BIN}-stderr.log" 2>&1
EXIT_CODE=$?

echo "Restoring UI..."
killall -CONT KPPMainApp 2>/dev/null
killall -CONT cvm 2>/dev/null
killall -CONT awesome 2>/dev/null
lipc-set-prop com.lab126.pillow disableEnablePillow enable 2>/dev/null

echo "Done (exit $EXIT_CODE)."
exit $EXIT_CODE
```

## Deploy script

Having a script for deploying the app is very handy. The example below builds your app, configures the USB-ethernet interface, and pushes the binary plus the launcher over SSH.

```sh
#!/bin/sh
set -eu

PACKAGE="my-app"
TARGET="armv7-unknown-linux-musleabihf"
KINDLE_HOST="${KINDLE_HOST:-kindle}"
KINDLE_DST="/mnt/us"

KINDLE_MAC="ee:49:00:00:00:00"
HOST_IP="192.168.15.201"
PREFIX="24"

IFACE=$(ip -o link | awk -v mac="$KINDLE_MAC" 'tolower($0) ~ mac { name=$2; sub(":", "", name); print name; exit }')

if [ -z "$IFACE" ]; then
    echo "error: no interface with MAC $KINDLE_MAC found (is USBNetwork enabled?)" >&2
    exit 1
fi

if ! ip -o addr show "$IFACE" | grep -q "inet $HOST_IP/"; then
    echo "Configuring $IFACE with $HOST_IP (sudo)..."
    sudo ip addr add "$HOST_IP/$PREFIX" dev "$IFACE"
    sudo ip link set "$IFACE" up
fi

echo "Building $PACKAGE for $TARGET..."
cargo zigbuild --release --target "$TARGET" -p "$PACKAGE"

BIN_SRC="target/$TARGET/release/$PACKAGE"
echo "Binary size: $(du -h "$BIN_SRC" | awk '{print $1}')"

echo "Copying to $KINDLE_HOST:$KINDLE_DST..."
scp "$BIN_SRC" "$KINDLE_HOST:$KINDLE_DST/$PACKAGE"
scp launch.sh "$KINDLE_HOST:$KINDLE_DST/launch-$PACKAGE.sh"

echo "Done. On the device: launch-$PACKAGE.sh"
```