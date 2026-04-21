//! User configuration.
//!
//! Configuration lives at `~/.config/copydeck/config.toml`.  All fields have
//! sensible defaults so the file is entirely optional — CopyDeck runs
//! correctly with no config file present.
//!
//! # Example config
//!
//! ```toml
//! [general]
//! history_limit = 300
//!
//! [ui]
//! theme = "dark"
//!
//! [hotkeys]
//! open_history   = "super+c"
//! open_and_paste = "super+shift+v"
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Top-level config ──────────────────────────────────────────────────────────

/// Complete CopyDeck configuration.
///
/// Every section implements [`Default`], so missing TOML keys fall back to
/// the defaults shown in each struct's documentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub hotkeys: HotkeyConfig,
    pub ui: UiConfig,
    pub storage: StorageConfig,
    pub paste: PasteConfig,
    pub monitor: MonitorConfig,
}

// ── Section structs ───────────────────────────────────────────────────────────

/// General behaviour settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Maximum number of items kept in the rolling history.
    ///
    /// Oldest entries are deleted automatically when this limit is exceeded.
    /// Default: `100`.
    pub history_limit: usize,

    /// Maximum number of pinned items kept.
    ///
    /// When pinning a new item beyond this limit, the oldest pinned item
    /// (by `pinned_at`) is automatically removed.  Default: `20`.
    pub pin_limit: usize,

    /// Skip clipboard entries larger than this many kilobytes.
    ///
    /// Prevents accidental storage of very large pastes (e.g. base64-encoded
    /// images).  Default: `512`.
    pub content_size_limit_kb: usize,
}

/// Global hotkey bindings.
///
/// Key combos are expressed as lowercase strings, e.g. `"super+c"`,
/// `"ctrl+shift+v"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    /// Open the clipboard history popup.  Default: `"super+c"`.
    pub open_history: String,

    /// Open the clipboard history popup and auto-paste the selected item.
    /// Default: `"super+shift+v"` — `Super+V` is reserved by GNOME (notification panel).
    pub open_and_paste: String,
}

/// Popup UI settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Colour scheme.  One of `"auto"`, `"dark"`, or `"light"`.
    /// `"auto"` follows the system preference.  Default: `"auto"`.
    pub theme: String,

    /// Width of the popup window in pixels.  Default: `580`.
    pub popup_width: u32,

    /// Height of the popup window in pixels.  Default: `700`.
    pub popup_height: u32,

    /// Maximum number of content lines shown per history row before
    /// truncating with `↵ (+N lines)`.  Default: `3`.
    pub max_preview_lines: usize,

    /// Pango font description for content previews.
    /// Default: `"Monospace 11"`.
    pub font: String,

    /// Show relative timestamps (e.g. "2m ago") next to each row.
    /// Default: `true`.
    pub show_timestamps: bool,
}

/// Storage settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Filesystem path to the SQLite database.
    ///
    /// A leading `~/` is expanded to the user's home directory.
    /// Default: `~/.local/share/copydeck/copydeck.db`.
    pub db_path: String,
}

/// Paste-injection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PasteConfig {
    /// Milliseconds to wait after restoring window focus before injecting the
    /// Ctrl+V keystroke.  Increase if pastes land in the wrong window.
    /// Default: `80`.
    pub focus_restore_delay_ms: u64,
}

/// Clipboard monitoring settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    /// How often to poll the clipboard for changes, in milliseconds.
    /// Default: `500`.
    pub poll_interval_ms: u64,

    /// Window class names whose clipboard activity is silently ignored.
    ///
    /// Use this to prevent password managers from leaking secrets into history.
    /// Default: `["gnome-keyring-dialog", "keepassxc", "1password"]`.
    pub exclude_apps: Vec<String>,
}

// ── Default implementations ───────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            hotkeys: HotkeyConfig::default(),
            ui: UiConfig::default(),
            storage: StorageConfig::default(),
            paste: PasteConfig::default(),
            monitor: MonitorConfig::default(),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            history_limit: 100,
            pin_limit: 20,
            content_size_limit_kb: 512,
        }
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            open_history: "super+c".to_owned(),
            // Super+V is a GNOME built-in (notification panel) and cannot be
            // overridden by custom shortcuts.  Super+Shift+V is conflict-free.
            open_and_paste: "super+shift+v".to_owned(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "auto".to_owned(),
            popup_width: 580,
            popup_height: 700,
            max_preview_lines: 3,
            font: "Monospace 13".to_owned(),
            show_timestamps: true,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
        }
    }
}

impl Default for PasteConfig {
    fn default() -> Self {
        // 300 ms: background-thread paste runs concurrently with GTK flushing
        // the window-hide to the compositor.  On Wayland/GNOME the compositor
        // needs at least one frame (~16 ms) to unmap the window and transfer
        // focus; 300 ms gives comfortable headroom plus ydotool's own 100 ms
        // built-in startup delay.
        Self {
            focus_restore_delay_ms: 300,
        }
    }
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 500,
            exclude_apps: vec![
                "gnome-keyring-dialog".to_owned(),
                "keepassxc".to_owned(),
                "1password".to_owned(),
            ],
        }
    }
}

// ── Config I/O ────────────────────────────────────────────────────────────────

impl Config {
    /// Load configuration from the default path
    /// (`~/.config/copydeck/config.toml`).
    ///
    /// A missing file is not an error — [`Config::default()`] is returned
    /// instead.  Returns an error only when the file exists but contains
    /// invalid TOML.
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::config_path())
    }

    /// Load configuration from an explicit path.
    ///
    /// Useful in tests where you want to supply a temporary config file.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;

        toml::from_str(&raw).with_context(|| format!("parsing config file {}", path.display()))
    }

    /// Write the current configuration to the default path, creating
    /// intermediate directories as needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating config directory {}", dir.display()))?;
        }

        let contents = toml::to_string_pretty(self).context("serialising config to TOML")?;

        std::fs::write(&path, contents)
            .with_context(|| format!("writing config file {}", path.display()))
    }

    /// Path to the user configuration file.
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("copydeck")
            .join("config.toml")
    }

    /// Resolve the database path, expanding a leading `~/` to the real home
    /// directory.
    pub fn resolved_db_path(&self) -> PathBuf {
        expand_tilde(&self.storage.db_path)
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn default_db_path() -> String {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("copydeck")
        .join("copydeck.db")
        .to_string_lossy()
        .into_owned()
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(rest)
    } else {
        PathBuf::from(path)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = Config::default();
        assert_eq!(cfg.general.history_limit, 100);
        assert_eq!(cfg.hotkeys.open_history, "super+c");
        assert_eq!(cfg.ui.theme, "auto");
        assert!(cfg.monitor.poll_interval_ms > 0);
    }

    #[test]
    fn load_from_missing_file_returns_default() {
        let path = PathBuf::from("/tmp/copydeck_nonexistent_config.toml");
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.general.history_limit, 100);
    }

    #[test]
    fn load_from_partial_toml_fills_missing_with_defaults() {
        let toml = "[ui]\ntheme = \"dark\"\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.ui.theme, "dark");
        // Unset fields use their defaults.
        assert_eq!(cfg.general.history_limit, 100);
        assert_eq!(cfg.hotkeys.open_and_paste, "super+shift+v");
    }

    #[test]
    fn expand_tilde_replaces_home() {
        let p = expand_tilde("~/foo/bar");
        assert!(!p.to_string_lossy().starts_with('~'));
        assert!(p.to_string_lossy().ends_with("foo/bar"));
    }

    #[test]
    fn expand_tilde_leaves_absolute_paths_unchanged() {
        let p = expand_tilde("/absolute/path");
        assert_eq!(p, PathBuf::from("/absolute/path"));
    }
}
