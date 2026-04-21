# CopyDeck

[![PyPI](https://img.shields.io/pypi/v/copydeck?label=pypi)](https://pypi.org/project/copydeck/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Made with Rust](https://img.shields.io/badge/made%20with-rust-orange)](https://www.rust-lang.org/)
[![GTK4](https://img.shields.io/badge/UI-GTK4-blue)](https://www.gtk.org/)

A lightweight clipboard manager for Linux with persistent pinned items, keyboard navigation, and format preservation.

- **~5 MB memory** at idle — Rust binary, no interpreter overhead
- **pip installable** — pre-compiled binary wheel, no Rust compiler needed
- **Ctrl+C and Super+C** both captured and shown in the same history list
- **Pinned items** persist across reboots
- **GTK4 popup** — keyboard-navigable, live search, dark/light theme

## Install

### One-liner (Debian / Ubuntu — recommended)

```bash
curl -fsSL https://github.com/rahul-anb/Copydeck/releases/latest/download/install.sh | bash
```

Downloads the pre-built `.deb` for your architecture, installs it with `apt`, and registers the service and hotkeys. Log out and back in when done.

### Manual .deb install

Download the latest `.deb` from the [Releases](https://github.com/rahul-anb/Copydeck/releases) page, then:

```bash
sudo dpkg -i copydeck_*_amd64.deb   # x86_64
sudo apt-get install -f              # pull in any missing runtime deps
copydeck install-service
```

### pip (any Linux)

```bash
pip install copydeck

# If your shell says "copydeck: command not found", add pip's bin directory to PATH:
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc && source ~/.bashrc

copydeck install-service
```

No Rust or compiler needed — pip downloads a pre-built binary wheel.

## After a reinstall

If you reinstall or upgrade CopyDeck, restart the daemon to pick up the new binary:

```bash
systemctl --user restart copydeck
```

**Pins and history are stored in** `~/.local/share/copydeck/copydeck.db` — back this file up to preserve your data across reinstalls.

## Hotkeys

| Key | Action |
|-----|--------|
| `Super+C` | Open clipboard history popup |
| `Super+Shift+V` | Open popup — selected item pastes immediately |
| `Ctrl+C` | Standard OS copy — automatically added to history |

Both `Ctrl+C` and `Super+C` copies appear in the same history list.

## Popup keyboard navigation

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move selection (crosses Pinned / Recent boundary) |
| `Tab` / `Shift+Tab` | Jump between Pinned and Recent sections |
| `Enter` | Paste selected item and close |
| `Ctrl+Enter` | Paste without closing (multi-paste mode) |
| `p` | Pin / unpin selected item |
| `Del` | Delete selected history entry |
| `Esc` | Close without pasting |

## Pinned items

Pinned items live above the rolling history and survive reboots.

```bash
# Pin something from the command line
copydeck pin add "SELECT * FROM users LIMIT 10" --label "Quick SQL"

# List all pins with their IDs
copydeck pin list

# Remove a pin by ID
copydeck pin remove 3

# Export / import (backup or share across machines)
copydeck pin export --output pins.json
copydeck pin import pins.json
```

## Daemon control

```bash
copydeck start          # start the daemon manually (normally handled by systemd)
copydeck pause          # pause clipboard monitoring (e.g. before entering a password)
copydeck resume         # resume monitoring after a pause
```

## Configuration

Config file: `~/.config/copydeck/config.toml`

The file is optional — all fields fall back to the defaults shown below.

```toml
[general]
history_limit         = 200    # rolling window size
content_size_limit_kb = 512    # ignore clipboard entries larger than this

[hotkeys]
open_history   = "super+c"
open_and_paste = "super+shift+v"

[ui]
theme             = "auto"   # "auto" | "dark" | "light"
popup_width       = 580
popup_height      = 700
max_preview_lines = 3        # lines shown per row before truncating
font              = "Monospace 13"
show_timestamps   = true

[storage]
db_path = "~/.local/share/copydeck/copydeck.db"

[paste]
focus_restore_delay_ms = 300  # increase if pastes land in the wrong window

[monitor]
poll_interval_ms = 500
# Clipboard activity from these apps is silently ignored
exclude_apps = ["gnome-keyring-dialog", "keepassxc", "1password"]
```

```bash
# Print current config
copydeck config

# Read a single value
copydeck config ui.theme

# Set a value
copydeck config ui.theme dark

# Check system dependencies
copydeck check-deps
```

## Wayland support

Global hotkeys are restricted by the Wayland security model.
`copydeck install-service` automatically registers `Super+C` / `Super+Shift+V` as
GNOME custom keyboard shortcuts via `gsettings`.

Alternatively, add these manually in **GNOME Settings → Keyboard → Custom Shortcuts**:

| Name | Command | Shortcut |
|------|---------|----------|
| CopyDeck Open | `copydeck open` | `Super+C` |
| CopyDeck Paste | `copydeck open --paste` | `Super+Shift+V` |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

Bug reports and feature requests are welcome — please use the
[issue tracker](https://github.com/rahul-anb/Copydeck/issues).

## Maintainer

CopyDeck is a personal side project maintained by
[Rahul Anbalagan](https://github.com/rahul-anb).
It started as a way to explore Linux daemons, IPC, and the Wayland
clipboard stack. Feedback and pull requests are welcome, but please note
this is not a funded project — response times may vary.

## Security

If you discover a security issue, please open a private advisory on the
GitHub repo rather than a public issue.

## License

[MIT](LICENSE)
