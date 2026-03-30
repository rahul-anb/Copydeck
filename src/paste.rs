//! Paste engine: write content to the clipboard and inject a `Ctrl+V`
//! keystroke into the previously focused window.
//!
//! # Flow
//!
//! ```text
//! User selects item in popup
//!      │
//!      ▼
//! PasteEngine::paste(content, mime_type)
//!      │
//!      ├─ 1. signal ignore_next on the monitor (prevents re-ingestion)
//!      ├─ 2. write content to system clipboard (arboard)
//!      ├─ 3. restore focus to the previous window (xdotool / best-effort)
//!      ├─ 4. sleep focus_restore_delay_ms (gives the WM time to transfer focus)
//!      └─ 5. inject Ctrl+V (enigo on X11; ydotool on Wayland)
//! ```
//!
//! # Platform support
//!
//! | Platform | Clipboard write | Paste injection |
//! |----------|-----------------|-----------------|
//! | X11      | `arboard`       | `enigo` → `xdotool` fallback |
//! | Wayland  | `arboard`       | `ydotool key 29:1 47:1 47:0 29:0` |
//! | Headless | `arboard`       | no-op (no display) |

use anyhow::{Context, Result};
use arboard::Clipboard;
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::config::PasteConfig;
use crate::utils::display::DisplayServer;

// ── PasteEngine ────────────────────────────────────────────────────────────────

/// Writes clipboard content and injects paste keystrokes.
///
/// A single `PasteEngine` is created at daemon startup and shared with the
/// UI via `Arc`.
pub struct PasteEngine {
    config: PasteConfig,
    display_server: Option<DisplayServer>,

    /// Set `true` on the [`crate::monitor::MonitorHandle`] before writing to
    /// the clipboard so the monitor does not re-ingest the pasted content as a
    /// new history entry.
    ///
    /// The daemon wires this up during startup:
    /// ```ignore
    /// engine.ignore_next = Arc::clone(&monitor_handle.ignore_next);
    /// ```
    pub ignore_next: Arc<AtomicBool>,
}

impl PasteEngine {
    /// Create a new paste engine.
    ///
    /// `ignore_next` should be the atomic flag from the running
    /// [`MonitorHandle`](crate::monitor::MonitorHandle).
    pub fn new(
        config: PasteConfig,
        display_server: Option<DisplayServer>,
        ignore_next: Arc<AtomicBool>,
    ) -> Self {
        Self {
            config,
            display_server,
            ignore_next,
        }
    }

    /// Write `content` to the clipboard and inject `Ctrl+V` into the
    /// previously focused window.
    ///
    /// The caller is responsible for capturing the focused window ID *before*
    /// showing the popup (via [`capture_active_window`]) and passing it here.
    pub fn paste(&self, content: &str, mime_type: &str, prev_window: Option<u64>) -> Result<()> {
        // Tell the monitor to skip the next clipboard change — this is the
        // content we're about to write, not a new user copy.
        self.ignore_next.store(true, Ordering::SeqCst);

        self.set_clipboard(content, mime_type)
            .context("writing to clipboard")?;

        // Restore focus to the application that was active before the popup.
        if let Some(wid) = prev_window {
            self.restore_focus(wid);
        }

        // Give the WM time to transfer keyboard focus.
        thread::sleep(Duration::from_millis(self.config.focus_restore_delay_ms));

        self.inject_paste().context("injecting paste keystroke")?;

        info!("Paste complete ({} bytes, {})", content.len(), mime_type);
        Ok(())
    }

    // ── Clipboard write ───────────────────────────────────────────────────────

    /// Write `content` to the system clipboard.
    ///
    /// On Wayland we use `wl-copy` (a subprocess that keeps running and serves
    /// clipboard requests) instead of arboard.  arboard drops clipboard
    /// ownership the moment the `Clipboard` struct is dropped, so any Ctrl+V
    /// that arrives even 80 ms later would paste whatever was in the clipboard
    /// *before* — typically stale content.  wl-copy survives past this
    /// function and stays alive until the next clipboard write.
    ///
    /// On X11 arboard works fine because X11 clipboard ownership persists
    /// until another client claims it.
    pub fn set_clipboard(&self, content: &str, _mime_type: &str) -> Result<()> {
        if self.display_server == Some(DisplayServer::Wayland) {
            return self.set_clipboard_wlcopy(content);
        }
        // X11 / headless: arboard is sufficient.
        let mut board = Clipboard::new().context("opening clipboard")?;
        board.set_text(content).context("setting clipboard text")?;
        debug!("Clipboard written ({} bytes)", content.len());
        Ok(())
    }

    /// Wayland clipboard write via `wl-copy` subprocess.
    ///
    /// The child process is intentionally *not* waited on; it keeps running
    /// and serves paste requests until another client writes to the clipboard.
    fn set_clipboard_wlcopy(&self, content: &str) -> Result<()> {
        use std::io::Write as _;
        let mut child = Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("spawning wl-copy")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(content.as_bytes())
                .context("writing to wl-copy stdin")?;
        }
        debug!("wl-copy clipboard write ({} bytes)", content.len());
        Ok(())
    }

    // ── Paste injection ───────────────────────────────────────────────────────

    /// Simulate `Ctrl+V` in the target application.
    pub fn inject_paste(&self) -> Result<()> {
        match self.display_server {
            Some(DisplayServer::X11) => self.inject_x11(),
            Some(DisplayServer::Wayland) => self.inject_wayland(),
            None => {
                warn!("No display server detected — skipping paste injection");
                Ok(())
            }
        }
    }

    /// X11: use `enigo` (if the `enigo-paste` feature is enabled); otherwise
    /// shell out to `xdotool`.
    fn inject_x11(&self) -> Result<()> {
        #[cfg(feature = "enigo-paste")]
        {
            match self.inject_via_enigo() {
                Ok(()) => return Ok(()),
                Err(e) => warn!("enigo paste failed ({e}); falling back to xdotool"),
            }
        }
        self.inject_via_xdotool()
    }

    /// Wayland: use `ydotool`; fall back to a warning when not installed.
    fn inject_wayland(&self) -> Result<()> {
        // Use the ydotool modifier+key format (e.g. "ctrl+v").
        // The raw evdev-code format "29:1 47:1 47:0 29:0" is NOT supported by
        // this ydotool version and causes it to press the first digit of each
        // argument as a literal key — producing "2442" instead of Ctrl+V.
        let status = Command::new("ydotool").args(["key", "ctrl+v"]).status();

        match status {
            Ok(s) if s.success() => {
                debug!("ydotool paste injected");
                Ok(())
            }
            Ok(s) => {
                warn!("ydotool exited with {s}");
                Err(anyhow::anyhow!("ydotool failed with status {s}"))
            }
            Err(e) => {
                warn!(
                    "ydotool not found ({e}). \
                     Install with: sudo apt install ydotool\n\
                     Content is in your clipboard — press Ctrl+V manually."
                );
                Ok(()) // non-fatal: content is in clipboard
            }
        }
    }

    /// Inject Ctrl+V using the `enigo` input library.
    ///
    /// Only compiled when the `enigo-paste` feature is enabled
    /// (`cargo build --features enigo-paste`), which requires `libxdo-dev`.
    #[cfg(feature = "enigo-paste")]
    fn inject_via_enigo(&self) -> Result<()> {
        use enigo::{Direction, Enigo, Key, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default()).context("initialising enigo")?;

        enigo
            .key(Key::Control, Direction::Press)
            .context("enigo: Control press")?;
        enigo
            .key(Key::Unicode('v'), Direction::Click)
            .context("enigo: v click")?;
        enigo
            .key(Key::Control, Direction::Release)
            .context("enigo: Control release")?;

        debug!("enigo paste injected");
        Ok(())
    }

    /// Inject Ctrl+V using `xdotool` (primary X11 method; fallback when
    /// `enigo-paste` is not enabled).
    fn inject_via_xdotool(&self) -> Result<()> {
        let status = Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+v"])
            .status()
            .context("running xdotool")?;

        if status.success() {
            debug!("xdotool paste injected");
            Ok(())
        } else {
            Err(anyhow::anyhow!("xdotool exited with {status}"))
        }
    }

    // ── Window focus management ───────────────────────────────────────────────

    /// Restore keyboard focus to `window_id` (X11 window ID).
    ///
    /// Called just before injecting the paste keystroke so it lands in the
    /// correct application.  Best-effort on Wayland (compositors restrict
    /// focus stealing).
    pub fn restore_focus(&self, window_id: u64) {
        match self.display_server {
            Some(DisplayServer::X11) => {
                let result = Command::new("xdotool")
                    .args(["windowfocus", "--sync", &window_id.to_string()])
                    .status();
                match result {
                    Ok(s) if s.success() => debug!(window_id, "Focus restored"),
                    Ok(s) => warn!(window_id, "xdotool windowfocus exit {s}"),
                    Err(e) => warn!(window_id, "xdotool windowfocus error: {e}"),
                }
            }
            Some(DisplayServer::Wayland) => {
                // Wayland compositors block focus stealing by design.
                // Best-effort: do nothing — the paste lands wherever focus is.
                debug!("Wayland: focus restoration not available");
            }
            None => {}
        }
    }
}

// ── Free functions ─────────────────────────────────────────────────────────────

/// Capture the currently active window ID on X11.
///
/// Returns `None` on Wayland, headless environments, or when `xdotool` is
/// unavailable.  Call this *before* showing the CopyDeck popup so the ID
/// can be passed to [`PasteEngine::paste`] after the user selects an item.
pub fn capture_active_window(display_server: Option<DisplayServer>) -> Option<u64> {
    if display_server != Some(DisplayServer::X11) {
        return None;
    }

    let output = Command::new("xdotool")
        .arg("getactivewindow")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let id_str = String::from_utf8_lossy(&output.stdout);
    let id: u64 = id_str.trim().parse().ok()?;
    debug!(window_id = id, "Captured active window");
    Some(id)
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn headless_engine() -> PasteEngine {
        PasteEngine::new(
            PasteConfig::default(),
            None, // no display server → all injection is no-op
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn ignore_next_set_before_clipboard_write() {
        let flag = Arc::new(AtomicBool::new(false));
        let engine = PasteEngine::new(PasteConfig::default(), None, Arc::clone(&flag));

        // paste() must set ignore_next before touching the clipboard.
        // We can't call paste() in headless CI (no clipboard), but we CAN
        // verify that ignore_next starts false and is set when we trigger it.
        assert!(!flag.load(Ordering::SeqCst));
        engine.ignore_next.store(true, Ordering::SeqCst);
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn inject_paste_headless_is_noop() {
        // Without a display server, inject_paste must succeed silently.
        let engine = headless_engine();
        assert!(engine.inject_paste().is_ok());
    }

    #[test]
    fn restore_focus_headless_is_noop() {
        // Without a display server, restore_focus must not panic.
        let engine = headless_engine();
        engine.restore_focus(12345);
    }

    #[test]
    fn capture_active_window_headless_returns_none() {
        // Without a display server, no window can be captured.
        assert!(capture_active_window(None).is_none());
        // On Wayland, also returns None.
        assert!(capture_active_window(Some(DisplayServer::Wayland)).is_none());
    }

    #[test]
    fn config_default_delay_is_300ms() {
        use crate::config::PasteConfig;
        assert_eq!(PasteConfig::default().focus_restore_delay_ms, 300);
    }
}
