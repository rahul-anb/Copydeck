//! CopyDeck — a lightweight clipboard manager for Linux.
//!
//! This crate exposes the core library used by the `copydeck` binary and by
//! integration tests.  The binary entry point lives in `src/main.rs`.
//!
//! # Module layout
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`config`] | Load / save user configuration from `~/.config/copydeck/config.toml` |
//! | [`daemon`] | Top-level daemon: wires monitor, IPC, hotkeys, UI |
//! | [`hotkeys`] | Global hotkey registration (`Super+C` / `Super+V`) |
//! | [`ipc`] | Unix socket IPC between the CLI and the running daemon |
//! | [`monitor`] | Background clipboard polling thread |
//! | [`paste`] | Write to clipboard and inject `Ctrl+V` |
//! | [`storage`] | SQLite-backed clipboard history and pinned items |
//! | [`utils`] | Display-server detection, system dependency checks |

pub mod config;
pub mod daemon;
pub mod hotkeys;
pub mod ipc;
pub mod monitor;
pub mod paste;
pub mod storage;
pub mod utils;

#[cfg(feature = "ui")]
pub mod ui;
