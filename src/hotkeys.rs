//! Global hotkey registration.
//!
//! Wraps [`global_hotkey::GlobalHotKeyManager`] to register `Super+C` and
//! `Super+V` (or user-configured combos) system-wide.
//!
//! # Platform behaviour
//!
//! | Platform | Mechanism | Notes |
//! |----------|-----------|-------|
//! | X11      | `XGrabKey` on the root window | No root needed; works with any WM |
//! | Wayland  | GlobalShortcuts portal (GNOME 45+, KDE 6+) | Falls back to dconf registration |
//!
//! # Usage
//!
//! Call [`HotkeyManager::register`] for each combo, then pass the returned
//! [`HotkeyManager`] to the GTK event loop.  Poll `GlobalHotKeyEvent::receiver()`
//! with `glib::timeout_add` to dispatch events to the daemon.
//!
//! ```no_run
//! use copydeck::hotkeys::{HotkeyManager, HotkeyAction};
//! use copydeck::config::HotkeyConfig;
//!
//! let cfg = HotkeyConfig::default();
//! let mut mgr = HotkeyManager::new().expect("hotkey init failed");
//! mgr.register(&cfg.open_history,   HotkeyAction::OpenHistory).unwrap();
//! mgr.register(&cfg.open_and_paste, HotkeyAction::OpenAndPaste).unwrap();
//! ```

use anyhow::{bail, Context, Result};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use std::collections::HashMap;
use tracing::info;

// ── HotkeyAction ──────────────────────────────────────────────────────────────

/// What happens when a registered hotkey fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Open the clipboard history popup (no auto-paste).
    OpenHistory,
    /// Open the popup and paste the selected item automatically.
    OpenAndPaste,
}

// ── HotkeyManager ─────────────────────────────────────────────────────────────

/// Manages global hotkey registrations for CopyDeck.
///
/// The manager owns a [`GlobalHotKeyManager`] and maintains a map from
/// hotkey IDs to [`HotkeyAction`]s so events can be dispatched.
pub struct HotkeyManager {
    inner:    GlobalHotKeyManager,
    actions:  HashMap<u32, HotkeyAction>,
    hotkeys:  Vec<HotKey>,
}

impl HotkeyManager {
    /// Initialise the global hotkey subsystem.
    ///
    /// # Errors
    ///
    /// Fails when no display server is available (headless environments) or
    /// when the underlying platform library cannot be initialised.
    pub fn new() -> Result<Self> {
        let inner = GlobalHotKeyManager::new()
            .context("initialising global hotkey manager")?;
        Ok(Self {
            inner,
            actions: HashMap::new(),
            hotkeys: Vec::new(),
        })
    }

    /// Register `combo` (e.g. `"super+c"`) and associate it with `action`.
    ///
    /// Combos are case-insensitive and tokens may be separated by `+`.
    /// Recognised modifiers: `ctrl`, `shift`, `alt`, `super`/`win`/`meta`.
    /// Key codes follow the US QWERTY layout (e.g. `c`, `v`, `f1`, `space`).
    ///
    /// # Errors
    ///
    /// Returns an error when the combo cannot be parsed or when the hotkey
    /// is already grabbed by another application.
    pub fn register(&mut self, combo: &str, action: HotkeyAction) -> Result<()> {
        let hotkey = parse_combo(combo)
            .with_context(|| format!("parsing hotkey combo {combo:?}"))?;

        self.inner
            .register(hotkey)
            .with_context(|| {
                format!(
                    "registering hotkey {combo:?} — another application may already hold this key grab"
                )
            })?;

        let id = hotkey.id();
        self.actions.insert(id, action);
        self.hotkeys.push(hotkey);
        info!(combo, ?action, "Hotkey registered");
        Ok(())
    }

    /// Unregister all hotkeys and release the grabs.
    pub fn unregister_all(&mut self) {
        // Best-effort: ignore errors (e.g. display disconnected on shutdown).
        let _ = self.inner.unregister_all(&self.hotkeys);
        self.hotkeys.clear();
        self.actions.clear();
    }

    /// Poll for the next pending hotkey event and translate it to the
    /// registered [`HotkeyAction`], if any.
    ///
    /// Returns `None` when there are no pending events or when the event ID
    /// is not in the registration map.  Only returns events with
    /// [`HotKeyState::Pressed`] (key-down); release events are ignored.
    pub fn try_next(&self) -> Option<HotkeyAction> {
        let event = GlobalHotKeyEvent::receiver().try_recv().ok()?;
        if event.state() != HotKeyState::Pressed {
            return None;
        }
        self.actions.get(&event.id()).copied()
    }
}

// ── Combo parser ───────────────────────────────────────────────────────────────

/// Parse a combo string like `"super+c"` into a [`HotKey`].
pub(crate) fn parse_combo(combo: &str) -> Result<HotKey> {
    let mut modifiers = Modifiers::empty();
    let mut key_code:  Option<Code> = None;

    for token in combo.split('+').map(str::trim).map(|s| s.to_ascii_lowercase()) {
        match token.as_str() {
            "ctrl"  | "control" => modifiers |= Modifiers::CONTROL,
            "shift"             => modifiers |= Modifiers::SHIFT,
            "alt"               => modifiers |= Modifiers::ALT,
            "super" | "win" | "meta" => modifiers |= Modifiers::SUPER,
            other => {
                if key_code.is_some() {
                    bail!("multiple key codes in combo {combo:?}");
                }
                key_code = Some(parse_key_code(other).with_context(|| {
                    format!("unrecognised key {other:?} in combo {combo:?}")
                })?);
            }
        }
    }

    let code = key_code.context("combo has no key code (e.g. 'super+c')")?;
    let mods = if modifiers.is_empty() { None } else { Some(modifiers) };
    Ok(HotKey::new(mods, code))
}

/// Map a single-key token to a [`Code`].
fn parse_key_code(token: &str) -> Result<Code> {
    let code = match token {
        // Letters
        "a" => Code::KeyA, "b" => Code::KeyB, "c" => Code::KeyC,
        "d" => Code::KeyD, "e" => Code::KeyE, "f" => Code::KeyF,
        "g" => Code::KeyG, "h" => Code::KeyH, "i" => Code::KeyI,
        "j" => Code::KeyJ, "k" => Code::KeyK, "l" => Code::KeyL,
        "m" => Code::KeyM, "n" => Code::KeyN, "o" => Code::KeyO,
        "p" => Code::KeyP, "q" => Code::KeyQ, "r" => Code::KeyR,
        "s" => Code::KeyS, "t" => Code::KeyT, "u" => Code::KeyU,
        "v" => Code::KeyV, "w" => Code::KeyW, "x" => Code::KeyX,
        "y" => Code::KeyY, "z" => Code::KeyZ,

        // Digits
        "0" => Code::Digit0, "1" => Code::Digit1, "2" => Code::Digit2,
        "3" => Code::Digit3, "4" => Code::Digit4, "5" => Code::Digit5,
        "6" => Code::Digit6, "7" => Code::Digit7, "8" => Code::Digit8,
        "9" => Code::Digit9,

        // Function keys
        "f1"  => Code::F1,  "f2"  => Code::F2,  "f3"  => Code::F3,
        "f4"  => Code::F4,  "f5"  => Code::F5,  "f6"  => Code::F6,
        "f7"  => Code::F7,  "f8"  => Code::F8,  "f9"  => Code::F9,
        "f10" => Code::F10, "f11" => Code::F11, "f12" => Code::F12,

        // Navigation / editing
        "space"     => Code::Space,
        "enter"     => Code::Enter,
        "tab"       => Code::Tab,
        "backspace" => Code::Backspace,
        "escape" | "esc" => Code::Escape,
        "delete" | "del" => Code::Delete,
        "insert"    => Code::Insert,
        "home"      => Code::Home,
        "end"       => Code::End,
        "pageup"    => Code::PageUp,
        "pagedown"  => Code::PageDown,
        "up"        => Code::ArrowUp,
        "down"      => Code::ArrowDown,
        "left"      => Code::ArrowLeft,
        "right"     => Code::ArrowRight,

        other => bail!("unknown key {other:?}"),
    };
    Ok(code)
}

// ── Wayland helpers ────────────────────────────────────────────────────────────

/// Print instructions for registering CopyDeck hotkeys on Wayland via
/// GNOME custom shortcuts (`gsettings`).
///
/// Called automatically by `copydeck --install-service` on Wayland.
pub fn print_wayland_setup_instructions() {
    println!(
        "Wayland detected. Global hotkeys require GNOME custom shortcuts.\n\
         Run the following commands to register them:\n\n\
         gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding \
             /org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/copydeck-open/ \
             name 'CopyDeck Open'\n\
         gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding \
             /org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/copydeck-open/ \
             command 'copydeck open'\n\
         gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding \
             /org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/copydeck-open/ \
             binding '<Super>c'\n\n\
         Or run: copydeck --setup-wayland"
    );
}

/// Register CopyDeck hotkeys as GNOME custom keyboard shortcuts via
/// `gsettings`.
///
/// Returns `Ok(())` on success.  Fails if `gsettings` is unavailable.
pub fn register_gnome_shortcuts() -> Result<()> {
    use std::process::Command;

    // Resolve the absolute path of the current binary.  GNOME custom shortcuts
    // are executed without sourcing ~/.profile or ~/.bashrc, so bare "copydeck"
    // is unresolvable.  Embedding the full path makes the shortcut work for all
    // users regardless of their $PATH configuration.
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("copydeck"));
    let exe_str = exe.to_string_lossy();

    let open_cmd  = format!("{exe_str} open");
    let paste_cmd = format!("{exe_str} open --paste");

    let shortcuts = [
        (
            "copydeck-open",
            "CopyDeck: Open clipboard history",
            open_cmd.as_str(),
            "<Super>c",
        ),
        (
            "copydeck-paste",
            "CopyDeck: Open and paste",
            paste_cmd.as_str(),
            "<Super><Shift>v",
        ),
    ];

    let base = "org.gnome.settings-daemon.plugins.media-keys";
    let custom_path =
        "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/";

    // Build the list of existing custom keybinding paths and append ours.
    let list_out = Command::new("gsettings")
        .args(["get", base, "custom-keybindings"])
        .output()
        .context("running gsettings")?;

    let mut existing: Vec<String> = parse_gsettings_list(
        &String::from_utf8_lossy(&list_out.stdout),
    );

    for (id, name, command, binding) in &shortcuts {
        let path = format!("{custom_path}{id}/");
        let full_key = format!("{base}.custom-keybinding:{path}");

        Command::new("gsettings")
            .args(["set", &full_key, "name", name])
            .status().context("gsettings set name")?;
        Command::new("gsettings")
            .args(["set", &full_key, "command", command])
            .status().context("gsettings set command")?;
        Command::new("gsettings")
            .args(["set", &full_key, "binding", binding])
            .status().context("gsettings set binding")?;

        if !existing.iter().any(|p| p == &path) {
            existing.push(path);
        }

        info!(%name, %binding, "GNOME shortcut registered");
    }

    // Write the updated list back.
    let list_value = format!(
        "[{}]",
        existing
            .iter()
            .map(|p| format!("'{p}'"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Command::new("gsettings")
        .args(["set", base, "custom-keybindings", &list_value])
        .status()
        .context("gsettings set custom-keybindings list")?;

    Ok(())
}

/// Parse a gsettings list value like `['a', 'b']` into `vec!["a", "b"]`.
///
/// Also handles the `@as []` type-annotation form that gsettings outputs for
/// empty arrays of strings.
fn parse_gsettings_list(s: &str) -> Vec<String> {
    // Strip optional "@as " type-annotation prefix (empty string array).
    let s = s.trim();
    let s = s.strip_prefix("@as ").unwrap_or(s);
    let s = s.trim().trim_start_matches('[').trim_end_matches(']').trim();
    if s.is_empty() {
        return vec![];
    }
    s.split(',')
        .map(|t| t.trim().trim_matches('\'').to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_super_c() {
        let hk = parse_combo("super+c").unwrap();
        assert_eq!(hk.mods, Modifiers::SUPER);
    }

    #[test]
    fn parse_super_v() {
        let hk = parse_combo("super+v").unwrap();
        assert_eq!(hk.mods, Modifiers::SUPER);
    }

    #[test]
    fn parse_ctrl_shift_s() {
        let hk = parse_combo("ctrl+shift+s").unwrap();
        let expected = Modifiers::CONTROL | Modifiers::SHIFT;
        assert_eq!(hk.mods, expected);
    }

    #[test]
    fn parse_is_case_insensitive() {
        let lower = parse_combo("super+c").unwrap();
        let upper = parse_combo("Super+C").unwrap();
        assert_eq!(lower.id(), upper.id());
    }

    #[test]
    fn parse_win_alias_works() {
        let hk = parse_combo("win+c").unwrap();
        assert_eq!(hk.mods, Modifiers::SUPER);
    }

    #[test]
    fn parse_meta_alias_works() {
        let hk = parse_combo("meta+v").unwrap();
        assert_eq!(hk.mods, Modifiers::SUPER);
    }

    #[test]
    fn parse_function_key() {
        let hk = parse_combo("ctrl+f5").unwrap();
        assert_eq!(hk.mods, Modifiers::CONTROL);
    }

    #[test]
    fn parse_unknown_key_errors() {
        assert!(parse_combo("super+@@@").is_err());
    }

    #[test]
    fn parse_no_key_code_errors() {
        assert!(parse_combo("super+ctrl").is_err());
    }

    #[test]
    fn parse_gsettings_list_values() {
        let input = "['path/a/', 'path/b/']";
        let list  = parse_gsettings_list(input);
        assert_eq!(list, vec!["path/a/", "path/b/"]);
    }

    #[test]
    fn parse_gsettings_empty_list() {
        assert!(parse_gsettings_list("@as []").is_empty());
        assert!(parse_gsettings_list("[]").is_empty());
    }
}
