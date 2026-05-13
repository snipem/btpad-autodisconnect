# btpad-autodisconnect

Automatically disconnects Bluetooth gamepads after a configurable idle timeout on Linux.

Monitors raw input events via `evdev`. When no meaningful input is seen for the configured
duration, it disconnects the controller via BlueZ over D-Bus. Runs as a daemon — it
reconnects and watches again as soon as the controller pairs again.

Works with any Bluetooth controller Linux exposes as an input device (DualShock 4,
DualSense, Xbox, etc.).

## Features

- Configurable idle timeout
- Stick-drift filter — ignores axis movement within ±10 % of the axis range from center
- Supports multiple controllers simultaneously, matched by MAC address
- No root required (see [Setup](#setup))
- Debug mode prints last-activity age and event name every second

## Installation

Download the latest static binary from the [releases page](../../releases/latest) and
place it somewhere on your `$PATH`:

```sh
curl -L https://github.com/snipem/btpad-autodisconnect/releases/latest/download/btpad-autodisconnect \
  -o ~/.local/bin/btpad-autodisconnect
chmod +x ~/.local/bin/btpad-autodisconnect
```

## Setup

Reading from `/dev/input/event*` requires membership in the `input` group. This is a
one-time step and avoids running the tool as root:

```sh
sudo usermod -aG input $USER
```

Log out and back in (or run `newgrp input` in the current shell) for the change to take
effect. Verify:

```sh
id   # should list "input" among your groups
```

The BlueZ disconnect call goes over D-Bus and works as a normal user for devices paired
to your own session.

## Usage

```
btpad-autodisconnect [OPTIONS]

Options:
  -t, --timeout <SECONDS>  Idle timeout before disconnecting [default: 600]
  -n, --name <NAME>        Device name substring to match, case-insensitive
                           [default: "Wireless Controller"]
      --debug              Print last-activity info every second
  -h, --help               Print help
```

### Examples

```sh
# Default: disconnect DualShock 4 / DualSense after 10 minutes idle
btpad-autodisconnect

# 5-minute timeout
btpad-autodisconnect --timeout 300

# Xbox controller
btpad-autodisconnect --name "Xbox"

# Debug mode with short timeout to verify it works
btpad-autodisconnect --debug --timeout 20
```

### Debug output

```
$ btpad-autodisconnect --debug --timeout 20
Watching for "Wireless Controller" — idle timeout: 20s
Found: Wireless Controller [AA:BB:CC:DD:EE:FF] → /org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF
[Wireless Controller [AA:BB:CC:DD:EE:FF]] last activity: 1s ago  ((none))  (timeout: 20s)
[Wireless Controller [AA:BB:CC:DD:EE:FF]] last activity: 2s ago  ((none))  (timeout: 20s)
[Wireless Controller [AA:BB:CC:DD:EE:FF]] last activity: 3s ago  ((none))  (timeout: 20s)
[Wireless Controller [AA:BB:CC:DD:EE:FF]] last activity: 0s ago  (ABSOLUTE(16) = 1)  (timeout: 20s)
...
[Wireless Controller [AA:BB:CC:DD:EE:FF]] idle for 20s — disconnecting...
[Wireless Controller [AA:BB:CC:DD:EE:FF]] disconnected.
Found: Wireless Controller [AA:BB:CC:DD:EE:FF] → /org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF
[Wireless Controller [AA:BB:CC:DD:EE:FF]] last activity: 1s ago  ((none))  (timeout: 20s)
...
```

The event name in parentheses shows which input caused the last activity reset — useful
for identifying stick-drift axes.

## Autostart with systemd

Create `~/.config/systemd/user/btpad-autodisconnect.service`:

```ini
[Unit]
Description=Bluetooth gamepad auto-disconnect
After=bluetooth.target

[Service]
ExecStart=%h/.local/bin/btpad-autodisconnect --timeout 600
Restart=on-failure

[Install]
WantedBy=default.target
```

Then enable and start it:

```sh
systemctl --user enable --now btpad-autodisconnect
```

## Build from source

Requires Rust stable.

```sh
git clone https://github.com/snipem/btpad-autodisconnect
cd btpad-autodisconnect
cargo build --release
# binary at target/release/btpad-autodisconnect
```
