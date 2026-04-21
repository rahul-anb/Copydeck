//! CopyDeck daemon.
//!
//! [`CopyDeckDaemon`] owns all long-running components and wires them
//! together:
//!
//! ```text
//!  ┌───────────────────────────────────────────────────────┐
//!  │                  CopyDeckDaemon                        │
//!  │                                                        │
//!  │  ClipboardMonitor ──► mpsc::Receiver ──► StorageManager│
//!  │                                                        │
//!  │  IpcServer ──► mpsc::Receiver ──► popup / pause / etc  │
//!  │                                                        │
//!  │  HotkeyManager ──► (GTK timeout_add poll)              │
//!  │                                                        │
//!  │  GTK4 event loop  (feature = "ui")                     │
//!  │  — or —                                                │
//!  │  Headless blocking loop  (no UI feature)               │
//!  └───────────────────────────────────────────────────────┘
//! ```
//!
//! # Single-instance enforcement
//!
//! On startup the daemon writes its PID to a lock file
//! (`~/.local/share/copydeck/copydeck.lock`).  If the lock file already
//! exists and the recorded PID is still alive, the daemon exits and instead
//! forwards the `open` IPC action to the already-running instance.

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::hotkeys::{HotkeyAction, HotkeyManager};
use crate::ipc::{IpcAction, IpcServer};
use crate::monitor::{ClipboardEvent, ClipboardMonitor};
use crate::paste::PasteEngine;
use crate::storage::StorageManager;
use crate::utils::display::DisplayServer;

// ── CopyDeckDaemon ────────────────────────────────────────────────────────────

/// The top-level daemon that owns all CopyDeck components.
pub struct CopyDeckDaemon {
    config: Config,
    db: Arc<Mutex<StorageManager>>,
    ds: Option<DisplayServer>,
}

impl CopyDeckDaemon {
    /// Initialise all components from `config`.
    ///
    /// Opens the database, creates the paste engine, verifies the display
    /// server.  Does **not** start the monitor or event loop — call
    /// [`run`](Self::run) for that.
    pub fn new(config: Config) -> Result<Self> {
        let ds = DisplayServer::detect();
        if ds.is_none() {
            warn!("No display server detected — running headlessly");
        } else {
            info!(display_server = %ds.unwrap(), "Display server detected");
        }

        let db_path = config.resolved_db_path();
        let db = StorageManager::open(&db_path)
            .with_context(|| format!("opening database {}", db_path.display()))?;
        info!("Database opened at {}", db_path.display());

        Ok(Self {
            config,
            db: Arc::new(Mutex::new(db)),
            ds,
        })
    }

    /// Run the daemon main loop.
    ///
    /// Blocks until the process receives `SIGTERM` or the IPC `quit` command.
    ///
    /// When compiled with the `ui` feature this starts the GTK4 event loop.
    /// Without `ui` this runs a simple blocking loop that still processes
    /// clipboard events and IPC commands (useful for headless integration
    /// tests and CI).
    pub fn run(self) -> Result<()> {
        enforce_single_instance(&self.config)?;

        // ── Clipboard monitor ──────────────────────────────────────────────
        let monitor = ClipboardMonitor::new(self.ds, &self.config.monitor);
        let (monitor_rx, monitor_handle) = monitor.start();

        // ── Paste engine ───────────────────────────────────────────────────
        let paste_engine = Arc::new(PasteEngine::new(
            self.config.paste.clone(),
            self.ds,
            Arc::clone(&monitor_handle.ignore_next),
        ));

        // ── IPC server ─────────────────────────────────────────────────────
        let socket_path = crate::ipc::default_socket_path();
        let ipc_server = IpcServer::bind(&socket_path)
            .with_context(|| format!("binding IPC socket {}", socket_path.display()))?;

        let (ipc_tx, ipc_rx) = mpsc::channel::<IpcAction>();
        let _ipc_thread = thread::Builder::new()
            .name("copydeck-ipc".to_owned())
            .spawn(move || {
                info!("IPC server listening on {}", socket_path.display());
                loop {
                    match ipc_server.accept_one() {
                        Ok(Some(action)) => {
                            if ipc_tx.send(action).is_err() {
                                break; // main thread exited
                            }
                        }
                        Ok(None) => {} // malformed message, continue
                        Err(e) => {
                            error!("IPC accept error: {e}");
                            break;
                        }
                    }
                }
            })
            .expect("failed to spawn IPC thread");

        // ── Hotkeys ────────────────────────────────────────────────────────
        let hotkey_manager = self.init_hotkeys();

        // ── GNOME shortcuts (Wayland) ──────────────────────────────────────
        // On Wayland, global_hotkey uses XGrabKey (X11), which is invisible to
        // native Wayland clients such as Chrome.  GNOME custom shortcuts fire
        // at the compositor layer regardless of which app is focused, so we
        // keep them in sync on every daemon start.  The call is idempotent.
        if self.ds == Some(DisplayServer::Wayland) {
            match crate::hotkeys::register_gnome_shortcuts() {
                Ok(()) => info!("GNOME custom shortcuts refreshed"),
                Err(e) => warn!("Could not refresh GNOME shortcuts: {e}"),
            }
        }

        // ── Monitoring pause flag ──────────────────────────────────────────
        let paused = Arc::new(AtomicBool::new(false));

        info!("CopyDeck daemon started");

        // ── Event loop ─────────────────────────────────────────────────────
        #[cfg(feature = "ui")]
        {
            self.run_gtk_loop(
                monitor_rx,
                monitor_handle,
                ipc_rx,
                paste_engine,
                hotkey_manager,
                paused,
            )
        }

        #[cfg(not(feature = "ui"))]
        {
            self.run_headless_loop(monitor_rx, ipc_rx, paste_engine, hotkey_manager, paused)
        }
    }

    // ── Hotkey initialisation ──────────────────────────────────────────────────

    fn init_hotkeys(&self) -> Option<HotkeyManager> {
        match HotkeyManager::new() {
            Err(e) => {
                warn!("Could not initialise hotkey manager: {e}");
                None
            }
            Ok(mut mgr) => {
                let cfg = &self.config.hotkeys;
                if let Err(e) = mgr.register(&cfg.open_history, HotkeyAction::OpenHistory) {
                    warn!("Could not register {}: {e}", cfg.open_history);
                }
                if let Err(e) = mgr.register(&cfg.open_and_paste, HotkeyAction::OpenAndPaste) {
                    warn!("Could not register {}: {e}", cfg.open_and_paste);
                }
                Some(mgr)
            }
        }
    }

    // ── Headless event loop (no GTK4) ─────────────────────────────────────────

    /// Blocking loop used when the `ui` feature is not compiled in.
    ///
    /// Processes clipboard events and IPC commands without showing any UI.
    /// Useful for running the daemon in a headless environment or for
    /// integration testing.
    #[cfg(not(feature = "ui"))]
    fn run_headless_loop(
        self,
        monitor_rx: mpsc::Receiver<ClipboardEvent>,
        ipc_rx: mpsc::Receiver<IpcAction>,
        _paste_engine: Arc<PasteEngine>,
        _hotkey_manager: Option<HotkeyManager>,
        paused: Arc<AtomicBool>,
    ) -> Result<()> {
        loop {
            // Drain monitor events.
            for event in monitor_rx.try_iter() {
                if paused.load(Ordering::Relaxed) {
                    debug!("Monitor paused — skipping event");
                    continue;
                }
                self.store_clipboard_event(event);
            }

            // Drain IPC events.
            for action in ipc_rx.try_iter() {
                match action {
                    IpcAction::Pause => {
                        paused.store(true, Ordering::Relaxed);
                        info!("Clipboard monitoring paused");
                    }
                    IpcAction::Resume => {
                        paused.store(false, Ordering::Relaxed);
                        info!("Clipboard monitoring resumed");
                    }
                    IpcAction::Open | IpcAction::OpenPaste => {
                        // No UI to open in headless mode.
                        warn!("Popup requested but UI feature is not compiled in");
                    }
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    // ── GTK4 event loop ───────────────────────────────────────────────────────

    #[cfg(feature = "ui")]
    fn run_gtk_loop(
        self,
        monitor_rx: mpsc::Receiver<ClipboardEvent>,
        monitor_handle: crate::monitor::MonitorHandle,
        ipc_rx: mpsc::Receiver<IpcAction>,
        paste_engine: Arc<PasteEngine>,
        hotkey_manager: Option<HotkeyManager>,
        paused: Arc<AtomicBool>,
    ) -> Result<()> {
        use gtk4::gio::ApplicationFlags;
        use gtk4::prelude::*;
        use gtk4::Application;
        // Use glib through gtk4 re-export so the version matches gtk4 0.7.
        use gtk4::glib;
        use std::cell::RefCell;
        use std::rc::Rc;

        // No application_id: we don't want GNOME Shell tracking us as a
        // launchable app — that causes a brief taskbar flash on every
        // clipboard event we process on the main thread.  Single-instance
        // is enforced separately by the IPC socket lock (see `IpcServer`).
        // NON_UNIQUE skips D-Bus name registration entirely.
        let app = Application::builder()
            .flags(ApplicationFlags::NON_UNIQUE)
            .build();

        let db = Arc::clone(&self.db);
        let ds = self.ds;
        let config = self.config.clone();

        // connect_activate is Fn (not FnOnce), so values that would be moved
        // into inner closures must be wrapped in Rc<RefCell<Option<T>>> and
        // created *outside* the Fn closure.  Only Rc::clone (a borrow) happens
        // inside the closure, which is allowed.
        let monitor_rx_cell = Rc::new(RefCell::new(Some(monitor_rx)));
        let ipc_rx_cell = Rc::new(RefCell::new(Some(ipc_rx)));
        let hk_mgr_cell = Rc::new(RefCell::new(hotkey_manager));

        app.connect_activate(move |app| {
            // Keep the GApplication alive even when no windows are visible.
            // Without this hold the event loop exits immediately because
            // the popup starts hidden.  The guard must be stored — dropping
            // it immediately releases the hold.
            let _hold = app.hold();

            // Build the popup but keep it hidden initially.
            let popup = crate::ui::popup::CopyDeckPopup::new(
                app,
                Arc::clone(&db),
                Arc::clone(&paste_engine),
                ds,
                &config.ui,
                config.general.pin_limit,
            );

            // Glib channel: bridge background monitor events to GTK thread.
            let (monitor_glib_tx, monitor_glib_rx) =
                glib::MainContext::channel::<ClipboardEvent>(glib::Priority::DEFAULT);

            // Spawn the monitor bridge thread once; take the receiver out of
            // the cell (subsequent activate calls, if any, are no-ops).
            if let Some(rx) = monitor_rx_cell.borrow_mut().take() {
                let tx = monitor_glib_tx.clone();
                std::thread::spawn(move || {
                    for event in rx {
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                });
            }

            // Process clipboard events on the GTK main thread.
            let db_clone = Arc::clone(&db);
            let paused_attach = Arc::clone(&paused);
            let history_limit = config.general.history_limit;
            monitor_glib_rx.attach(None, move |event| {
                if paused_attach.load(Ordering::Relaxed) {
                    return glib::ControlFlow::Continue;
                }
                if let Ok(mut db) = db_clone.lock() {
                    let _ = db.add_history(
                        &event.content,
                        &event.mime_type,
                        event.source,
                        history_limit,
                    );
                }
                glib::ControlFlow::Continue
            });

            // Poll IPC + hotkeys at 50 ms intervals.
            let popup_ref = popup.clone();
            let popup_ref2 = popup.clone();
            let ipc_cell_ref = Rc::clone(&ipc_rx_cell);
            let hk_cell_ref = Rc::clone(&hk_mgr_cell);
            let paused_timeout = Arc::clone(&paused);

            glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                // IPC
                {
                    let borrow = ipc_cell_ref.borrow();
                    if let Some(rx) = borrow.as_ref() {
                        for action in rx.try_iter() {
                            match action {
                                IpcAction::Open => popup_ref.show(false),
                                IpcAction::OpenPaste => popup_ref.show(true),
                                IpcAction::Pause => paused_timeout.store(true, Ordering::Relaxed),
                                IpcAction::Resume => paused_timeout.store(false, Ordering::Relaxed),
                            }
                        }
                    }
                }

                // Hotkeys
                {
                    let borrow = hk_cell_ref.borrow();
                    if let Some(mgr) = borrow.as_ref() {
                        while let Some(action) = mgr.try_next() {
                            match action {
                                HotkeyAction::OpenHistory => popup_ref2.show(false),
                                HotkeyAction::OpenAndPaste => popup_ref2.show(true),
                            }
                        }
                    }
                }

                glib::ControlFlow::Continue
            });
        });

        let _monitor_handle = monitor_handle; // keep alive until app exits
                                              // Pass only the program name; omit subcommand args so GApplication
                                              // does not try to "open" them as files and exit immediately.
        app.run_with_args(&["copydeck"]);
        Ok(())
    }

    // ── Shared helpers ────────────────────────────────────────────────────────

    /// Persist a clipboard event to storage.
    fn store_clipboard_event(&self, event: ClipboardEvent) {
        let limit = self.config.general.history_limit;
        match self.db.lock() {
            Ok(db) => match db.add_history(&event.content, &event.mime_type, event.source, limit) {
                Ok(Some(id)) => debug!(id, "History entry added"),
                Ok(None) => debug!("Clipboard event deduplicated"),
                Err(e) => error!("Failed to store clipboard event: {e}"),
            },
            Err(e) => error!("DB lock poisoned: {e}"),
        }
    }
}

// ── Single-instance lock ──────────────────────────────────────────────────────

/// Path to the PID lock file.
pub fn lock_file_path() -> PathBuf {
    crate::ipc::default_socket_path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("/tmp"))
        .join("copydeck.lock")
}

/// Check if a PID from a lock file is still running.
fn pid_is_alive(pid: u32) -> bool {
    // On Linux, send signal 0 to check if the process exists.
    libc_kill(pid as i32, 0) == 0
}

/// Thin wrapper around the libc `kill(2)` syscall.
fn libc_kill(pid: i32, sig: i32) -> i32 {
    // Safety: kill(2) with signal 0 is always safe — it only checks existence.
    libc_kill_inner(pid, sig)
}

#[cfg(target_family = "unix")]
fn libc_kill_inner(pid: i32, sig: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid, sig) }
}

/// Write PID lock; forward to existing daemon and exit if already running.
fn enforce_single_instance(_config: &Config) -> Result<()> {
    let lock = lock_file_path();

    if lock.exists() {
        if let Ok(content) = fs::read_to_string(&lock) {
            if let Ok(pid) = content.trim().parse::<u32>() {
                if pid_is_alive(pid) {
                    info!("Another instance is running (PID {pid}); forwarding open");
                    // Forward: attempt to open the popup in the running instance.
                    let _ = crate::ipc::IpcClient::with_default_path()
                        .send(crate::ipc::IpcAction::Open);
                    std::process::exit(0);
                }
            }
        }
        // Stale lock — overwrite it.
    }

    let pid = std::process::id();
    if let Some(parent) = lock.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating lock directory {}", parent.display()))?;
    }
    let mut f = fs::File::create(&lock)
        .with_context(|| format!("creating lock file {}", lock.display()))?;
    write!(f, "{pid}").context("writing PID to lock file")?;
    debug!(pid, "PID lock written to {}", lock.display());
    Ok(())
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_file_path_is_in_data_dir() {
        let p = lock_file_path();
        assert!(
            p.to_string_lossy().contains("copydeck"),
            "lock path must mention copydeck: {}",
            p.display()
        );
        assert_eq!(
            p.file_name().and_then(|n| n.to_str()),
            Some("copydeck.lock")
        );
    }

    #[test]
    fn daemon_new_headless_succeeds() {
        // Verify CopyDeckDaemon::new() works with default config in CI.
        let cfg = Config::default();
        // Use /tmp to avoid touching the real data dir.
        let mut cfg = cfg;
        cfg.storage.db_path = "/tmp/copydeck_daemon_test.db".to_owned();
        CopyDeckDaemon::new(cfg).expect("daemon init must succeed in headless env");
    }
}
