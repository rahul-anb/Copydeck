# CopyDeck

A lightweight clipboard manager for Linux with persistent pinned items, keyboard navigation, and format preservation.

- **~5 MB memory** at idle — Rust binary, no interpreter overhead
- **pip installable** — pre-compiled binary wheel, no Rust compiler needed
- **Ctrl+C and Super+C** both captured and shown in the same history list
- **Pinned items** persist across reboots
- **GTK4 popup** — keyboard-navigable, live search, dark/light theme

## Quick start

```bash
# 1. Install system runtime libraries (pre-installed on Ubuntu 22+ desktop)
sudo apt install libgtk-4-1 xdotool

# 2. Install CopyDeck
pip install copydeck

# 3. Register the autostart service and hotkeys
copydeck install-service

# Log out and back in — CopyDeck starts automatically.
```

## Fresh system setup

Use this checklist when setting up CopyDeck on a new machine.

**1. System dependencies**

```bash
sudo apt install libgtk-4-1 xdotool ydotool
```

> `ydotool` is required for input injection on Wayland. On X11 `xdotool` alone is enough.

**2. Python 3.10+**

```bash
python3 --version   # must be 3.10 or newer
pip install --upgrade pip
```

**3. Install CopyDeck**

```bash
pip install copydeck
```

**4. Register the service and hotkeys**

```bash
copydeck install-service
```

This registers the systemd user service and sets `Super+C` / `Super+V` as GNOME keyboard shortcuts automatically.

**5. Verify**

```bash
copydeck check-deps     # confirms all runtime libraries are present
systemctl --user status copydeck   # should show "active (running)"
```

**6. After a system update or reinstall**

If you reinstall CopyDeck or the binary changes, restart the daemon so the running process picks up the new binary:

```bash
systemctl --user restart copydeck
```

**Pins and history are stored in** `~/.local/share/copydeck/history.db` — back this file up to preserve your pins across reinstalls.

## Hotkeys

| Key | Action |
|-----|--------|
| `Super+C` | Open clipboard history popup |
| `Super+V` | Open popup — selected item pastes immediately |
| `Ctrl+C`  | Standard OS copy — automatically added to history |

Both `Ctrl+C` and `Super+C` copies appear in the same history list.

## Popup keyboard navigation

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move selection (crosses Pinned / Recent boundary) |
| `Enter` | Paste selected item and close |
| `Ctrl+Enter` | Paste without closing (multi-paste mode) |
| `p` | Pin / unpin selected item |
| `r` | Rename selected pinned item |
| `Del` | Delete selected history entry |
| `Esc` | Close without pasting |

## Pinned items

Pinned items live above the rolling history and survive reboots.

```bash
# Pin something from the command line
copydeck pin add "SELECT * FROM users LIMIT 10" --label "Quick SQL"

# List pins
copydeck pin list

# Export / import (backup or share across machines)
copydeck pin export --output pins.json
copydeck pin import pins.json
```

## Configuration

Config file: `~/.config/copydeck/config.toml`

```toml
[general]
history_limit = 200          # rolling window size

[hotkeys]
open_history   = "super+c"
open_and_paste = "super+v"

[ui]
theme = "auto"               # "auto" | "dark" | "light"

[monitor]
# Clipboard activity from these apps is silently ignored
exclude_apps = ["gnome-keyring-dialog", "keepassxc", "1password"]
```

```bash
# Print current config
copydeck config

# Check system dependencies
copydeck check-deps
```

## Wayland support

Global hotkeys are restricted by the Wayland security model.
`copydeck install-service` automatically registers `Super+C` / `Super+V` as
GNOME custom keyboard shortcuts via `gsettings`.

Alternatively, add these manually in **GNOME Settings → Keyboard → Custom Shortcuts**:

| Name | Command | Shortcut |
|------|---------|----------|
| CopyDeck Open | `copydeck open` | `Super+C` |
| CopyDeck Paste | `copydeck open --paste` | `Super+V` |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
