//! IPC: Unix domain socket for daemon ↔ CLI communication.
//!
//! The daemon runs an [`IpcServer`] in a dedicated thread; CLI subcommands
//! (`open`, `pause`, `resume`) use [`IpcClient`] to forward requests.
//!
//! Socket path: `~/.local/share/copydeck/copydeck.sock`
//!
//! Wire protocol: newline-terminated JSON messages:
//! ```text
//! {"action":"open"}
//! {"action":"open_paste"}
//! {"action":"pause"}
//! {"action":"resume"}
//! ```
//!
//! Both sides understand only a single message per connection.  The client
//! writes one line and closes the connection; the server reads one line per
//! accepted connection.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

// ── IPC action ─────────────────────────────────────────────────────────────────

/// A request sent from the CLI to the running daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcAction {
    /// Open the clipboard history popup (read-only; no auto-paste).
    Open,
    /// Open the popup; paste the selected item into the previously focused window.
    OpenPaste,
    /// Pause clipboard monitoring until a [`Resume`](IpcAction::Resume) is received.
    Pause,
    /// Resume clipboard monitoring after a [`Pause`](IpcAction::Pause).
    Resume,
}

// ── Wire message wrapper ───────────────────────────────────────────────────────

/// On-wire JSON envelope: `{"action":"<variant>"}`.
#[derive(Serialize, Deserialize)]
struct IpcMessage {
    action: IpcAction,
}

// ── Socket path ────────────────────────────────────────────────────────────────

/// Default Unix socket path: `~/.local/share/copydeck/copydeck.sock`.
pub fn default_socket_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("copydeck")
        .join("copydeck.sock")
}

// ── IpcClient ──────────────────────────────────────────────────────────────────

/// Sends a single [`IpcAction`] to the running daemon.
///
/// # Errors
///
/// Returns an error when no daemon is running (socket not found) or
/// when the send fails.
pub struct IpcClient {
    socket_path: PathBuf,
}

impl IpcClient {
    /// Create a client targeting `socket_path`.
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Create a client targeting the [default socket path](default_socket_path).
    pub fn with_default_path() -> Self {
        Self::new(default_socket_path())
    }

    /// Connect to the daemon and send `action`.
    pub fn send(&self, action: IpcAction) -> Result<()> {
        let mut stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!(
                "CopyDeck daemon is not running (socket: {}). \
                 Start it with `copydeck start`.",
                self.socket_path.display()
            )
        })?;

        let msg = serde_json::to_string(&IpcMessage { action })
            .context("serialising IPC action")?;
        writeln!(stream, "{msg}").context("sending IPC message to daemon")?;
        debug!("IPC action sent");
        Ok(())
    }
}

// ── IpcServer ──────────────────────────────────────────────────────────────────

/// Listens on a Unix socket and decodes incoming [`IpcAction`] messages.
///
/// The server is normally run in a dedicated background thread inside the
/// daemon.  Call [`accept_one`](Self::accept_one) in a loop to receive
/// actions.
pub struct IpcServer {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl IpcServer {
    /// Bind to `socket_path`, removing any stale socket file first.
    ///
    /// Creates the parent directory if it does not exist.
    pub fn bind(socket_path: &Path) -> Result<Self> {
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating socket directory {}", parent.display())
            })?;
        }
        // Remove a stale socket left by a previously crashed daemon.
        let _ = std::fs::remove_file(socket_path);

        let listener = UnixListener::bind(socket_path)
            .with_context(|| format!("binding Unix socket {}", socket_path.display()))?;

        Ok(Self {
            listener,
            socket_path: socket_path.to_owned(),
        })
    }

    /// Block until a client connects, then read and return its [`IpcAction`].
    ///
    /// Returns `Ok(None)` when the client sent an empty or unrecognised
    /// message (the caller should log and loop rather than abort).
    pub fn accept_one(&self) -> Result<Option<IpcAction>> {
        let (stream, _addr) = self
            .listener
            .accept()
            .context("accepting IPC connection")?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).context("reading IPC message")?;
        let trimmed = line.trim();

        if trimmed.is_empty() {
            warn!("Received empty IPC message; ignoring");
            return Ok(None);
        }

        match serde_json::from_str::<IpcMessage>(trimmed) {
            Ok(msg) => {
                debug!(action = ?msg.action, "IPC action received");
                Ok(Some(msg.action))
            }
            Err(e) => {
                warn!("Failed to parse IPC message {:?}: {e}", trimmed);
                Ok(None)
            }
        }
    }
}

impl Drop for IpcServer {
    /// Remove the socket file when the server is dropped.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    fn unique_socket_path() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        PathBuf::from(format!("/tmp/copydeck_ipc_test_{n}.sock"))
    }

    fn roundtrip(action: IpcAction) {
        let path   = unique_socket_path();
        let server = IpcServer::bind(&path).expect("bind failed");

        let client_path = path.clone();
        let expected    = action.clone();
        let t = thread::spawn(move || {
            IpcClient::new(client_path)
                .send(expected)
                .expect("client send failed");
        });

        let received = server.accept_one().expect("accept failed");
        assert_eq!(received, Some(action));
        t.join().expect("client thread panicked");
    }

    #[test]
    fn send_open()       { roundtrip(IpcAction::Open); }

    #[test]
    fn send_open_paste() { roundtrip(IpcAction::OpenPaste); }

    #[test]
    fn send_pause()      { roundtrip(IpcAction::Pause); }

    #[test]
    fn send_resume()     { roundtrip(IpcAction::Resume); }

    #[test]
    fn socket_file_removed_on_server_drop() {
        let path = unique_socket_path();
        {
            let _server = IpcServer::bind(&path).expect("bind failed");
            assert!(path.exists(), "socket must exist while server is alive");
        }
        assert!(!path.exists(), "socket must be removed after server drop");
    }

    #[test]
    fn client_errors_when_daemon_not_running() {
        let path = PathBuf::from("/tmp/copydeck_ipc_no_daemon.sock");
        let _ = std::fs::remove_file(&path); // ensure socket absent
        let result = IpcClient::new(path).send(IpcAction::Open);
        assert!(result.is_err(), "should fail when no daemon is running");
    }

    #[test]
    fn server_ignores_malformed_message() {
        let path   = unique_socket_path();
        let server = IpcServer::bind(&path).expect("bind failed");

        let client_path = path.clone();
        thread::spawn(move || {
            let mut stream = UnixStream::connect(&client_path).unwrap();
            writeln!(stream, "not valid json").unwrap();
        });

        let result = server.accept_one().expect("accept must not error");
        assert_eq!(result, None, "malformed message must return None");
    }

    #[test]
    fn ipc_action_serialises_as_snake_case() {
        let cases = [
            (IpcAction::Open,      r#"{"action":"open"}"#),
            (IpcAction::OpenPaste, r#"{"action":"open_paste"}"#),
            (IpcAction::Pause,     r#"{"action":"pause"}"#),
            (IpcAction::Resume,    r#"{"action":"resume"}"#),
        ];
        for (action, expected) in &cases {
            let json = serde_json::to_string(&IpcMessage { action: action.clone() }).unwrap();
            assert_eq!(&json, expected, "action {:?}", action);
        }
    }

    #[test]
    fn ipc_action_deserialises_from_snake_case() {
        let cases = [
            (r#"{"action":"open"}"#,       IpcAction::Open),
            (r#"{"action":"open_paste"}"#, IpcAction::OpenPaste),
            (r#"{"action":"pause"}"#,      IpcAction::Pause),
            (r#"{"action":"resume"}"#,     IpcAction::Resume),
        ];
        for (json, expected) in &cases {
            let msg: IpcMessage = serde_json::from_str(json).unwrap();
            assert_eq!(msg.action, *expected, "json {json}");
        }
    }
}
