# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-03-28

### Added
- Clipboard monitoring via `arboard` with configurable poll interval (default 500 ms).
- SHA-256 deduplication — consecutive identical clips produce a single history entry.
- SQLite-backed storage (`rusqlite` with bundled SQLite) for history and pinned items.
- Source attribution — distinguishes `Ctrl+C` from `Super+C` copies.
- MIME-type tracking (`text/plain`, `text/html`, `text/uri-list`, …).
- Global hotkeys (`Super+C` → open history, `Super+V` → open and auto-paste).
- GTK4 popup UI (optional, behind the `ui` Cargo feature) with:
  - Live fuzzy search.
  - Pinned items list with drag-to-reorder.
  - Clipboard history list with relative timestamps and source badges.
  - Keyboard navigation (`↑↓`, `Enter`, `Ctrl+Enter`, `p`, `Del`, `Esc`).
  - Hint bar with keyboard shortcuts.
- Paste injection via `xdotool` (X11) or `ydotool` (Wayland).
- IPC via Unix domain socket — `open`, `open-paste`, `pause`, `resume` actions.
- `systemd --user` service with `install-service` sub-command.
- GNOME Wayland shortcut registration via `gsettings`.
- Single-instance enforcement via PID lock file.
- CLI sub-commands: `start`, `stop`, `open`, `pause`, `resume`, `pin`, `history`, `install-service`.
- Distributable as a Python wheel via `maturin` (no Python runtime required at run time).

[Unreleased]: https://github.com/your-org/copydeck/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/your-org/copydeck/releases/tag/v0.1.0
