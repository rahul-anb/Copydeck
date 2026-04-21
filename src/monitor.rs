//! Clipboard monitor.
//!
//! Runs a background thread that polls the system clipboard every
//! [`MonitorConfig::poll_interval_ms`] milliseconds.  On each change a
//! [`ClipboardEvent`] is sent over an [`mpsc`] channel so the daemon can
//! store it via the storage layer.
//!
//! # Design
//!
//! ```text
//!  ┌─────────────────────────────────────────────┐
//!  │  Background thread (ClipboardMonitor)        │
//!  │                                              │
//!  │  arboard::Clipboard::get_text()  ──────────▶ SHA-256
//!  │          (fast, in-process)       same?  no │
//!  │                                        yes  │
//!  │                                   ▼         │
//!  │                     enrich_with_mime()       │
//!  │                 (subprocess, only on change) │
//!  │                           │                  │
//!  │                           ▼                  │
//!  │                  mpsc::Sender<ClipboardEvent>│
//!  └───────────────────────────┼──────────────────┘
//!                              │
//!                    mpsc::Receiver<ClipboardEvent>  (daemon / storage)
//! ```
//!
//! Both `Ctrl+C` and `Super+C` produce the same clipboard change; the monitor
//! attributes the source by reading an atomic flag set by the hotkey handler.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::config::MonitorConfig;
use crate::storage::CopySource;
use crate::utils::display::DisplayServer;

// ── Public types ──────────────────────────────────────────────────────────────

/// An event emitted whenever the clipboard gains new content.
///
/// Both `Ctrl+C` and `Super+C` copies produce a `ClipboardEvent`; the
/// [`source`](ClipboardEvent::source) field distinguishes them.
#[derive(Debug, Clone)]
pub struct ClipboardEvent {
    /// Full clipboard content (UTF-8).
    pub content: String,

    /// MIME type of the content, e.g. `"text/plain"` or `"text/html"`.
    ///
    /// `text/html` is preferred when the source application offers it alongside
    /// plain text, allowing format-preserving pastes.
    pub mime_type: String,

    /// How this content was placed on the clipboard.
    pub source: CopySource,
}

/// A handle to a running clipboard monitor.
///
/// Dropping the handle sends a stop signal to the background thread.  The
/// thread exits cleanly on the next poll cycle.
pub struct MonitorHandle {
    /// Set to `true` to have the monitor skip the **next** detected change.
    ///
    /// The paste engine sets this flag before writing to the clipboard, so the
    /// content it pastes does not get re-added to history as a new entry.
    pub ignore_next: Arc<AtomicBool>,

    /// Set to `true` by the hotkey handler when `Super+C` is pressed.
    ///
    /// The monitor clears this flag once it reads the resulting clipboard
    /// change and assigns [`CopySource::SuperC`] to the event.  Hotkey
    /// handler: `handle.super_c_pressed.store(true, Ordering::SeqCst)`.
    pub super_c_pressed: Arc<AtomicBool>,

    // Stop flag — the thread exits when this is true.
    stop: Arc<AtomicBool>,

    _thread: JoinHandle<()>,
}

impl MonitorHandle {
    /// Signal the paste engine's intent: the next clipboard write should be
    /// ignored so it does not create a history entry.
    pub fn signal_ignore_next(&self) {
        self.ignore_next.store(true, Ordering::SeqCst);
    }

    /// Tell the monitor the next change came from `Super+C` (not `Ctrl+C`).
    ///
    /// Called by the hotkey handler immediately before the keystroke is
    /// forwarded to the application.
    pub fn signal_super_c(&self) {
        self.super_c_pressed.store(true, Ordering::SeqCst);
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// The clipboard monitor.
///
/// # Examples
///
/// ```no_run
/// use copydeck::monitor::ClipboardMonitor;
/// use copydeck::config::MonitorConfig;
/// use copydeck::utils::display::DisplayServer;
///
/// let (rx, _handle) = ClipboardMonitor::new(
///     DisplayServer::detect(),
///     &MonitorConfig::default(),
/// )
/// .start();
///
/// for event in rx {
///     println!("Copied [{}]: {}", event.mime_type, &event.content[..40.min(event.content.len())]);
/// }
/// ```
pub struct ClipboardMonitor {
    display_server: Option<DisplayServer>,
    poll_interval: Duration,
}

impl ClipboardMonitor {
    /// Create a new monitor configuration.
    ///
    /// Call [`start`](Self::start) to launch the background thread.
    pub fn new(display_server: Option<DisplayServer>, config: &MonitorConfig) -> Self {
        Self {
            display_server,
            poll_interval: Duration::from_millis(config.poll_interval_ms),
        }
    }

    /// Start the background polling thread.
    ///
    /// Returns `(receiver, handle)`.  Events flow over `receiver`.  The thread
    /// stops when `handle` is dropped or when the receiver is dropped.
    pub fn start(self) -> (mpsc::Receiver<ClipboardEvent>, MonitorHandle) {
        let reader: Box<dyn ClipboardReader> = match ArboardReader::try_new() {
            Some(r) => Box::new(r),
            None => {
                warn!("Could not open clipboard; monitoring disabled");
                Box::new(NullReader)
            }
        };

        self.start_with_reader(reader)
    }

    /// Start with a custom [`ClipboardReader`].
    ///
    /// Used in tests to inject a [`MockReader`] without requiring a real
    /// display server.  Also callable from integration tests.
    pub fn start_with_reader(
        self,
        reader: Box<dyn ClipboardReader>,
    ) -> (mpsc::Receiver<ClipboardEvent>, MonitorHandle) {
        let (tx, rx) = mpsc::channel();

        let ignore_next = Arc::new(AtomicBool::new(false));
        let super_c_pressed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_ignore = Arc::clone(&ignore_next);
        let thread_super_c = Arc::clone(&super_c_pressed);
        let thread_stop = Arc::clone(&stop);
        let ds = self.display_server;
        let interval = self.poll_interval;

        let thread = thread::Builder::new()
            .name("copydeck-monitor".to_owned())
            .spawn(move || {
                poll_loop(
                    reader,
                    tx,
                    thread_ignore,
                    thread_super_c,
                    thread_stop,
                    ds,
                    interval,
                );
            })
            .expect("failed to spawn clipboard monitor thread");

        let handle = MonitorHandle {
            ignore_next,
            super_c_pressed,
            stop,
            _thread: thread,
        };

        (rx, handle)
    }
}

// ── ClipboardReader trait ─────────────────────────────────────────────────────

/// Abstraction over clipboard reading.
///
/// Allows the production [`ArboardReader`] and the test [`MockReader`] to be
/// used interchangeably, without requiring a real display server in tests.
/// Also implemented by custom readers in integration tests.
pub trait ClipboardReader: Send + 'static {
    /// Return the current clipboard text, or `None` if the clipboard is empty
    /// or inaccessible.
    fn read_text(&mut self) -> Option<String>;
}

// ── ArboardReader — production ────────────────────────────────────────────────

struct ArboardReader {
    clipboard: arboard::Clipboard,
}

impl ArboardReader {
    fn try_new() -> Option<Self> {
        arboard::Clipboard::new()
            .map(|clipboard| Self { clipboard })
            .map_err(|e| warn!("arboard init error: {e}"))
            .ok()
    }
}

impl ClipboardReader for ArboardReader {
    fn read_text(&mut self) -> Option<String> {
        match self.clipboard.get_text() {
            Ok(t) if t.is_empty() => None,
            Ok(t) => Some(t),
            Err(e) => {
                debug!("arboard read_text error: {e}");
                None
            }
        }
    }
}

// ── NullReader — fallback when no display is detected ────────────────────────

/// A no-op reader used when clipboard access is unavailable (e.g. headless CI).
struct NullReader;

impl ClipboardReader for NullReader {
    fn read_text(&mut self) -> Option<String> {
        None
    }
}

// ── MockReader — test helper ──────────────────────────────────────────────────

#[cfg(test)]
use std::collections::VecDeque;

/// A deterministic reader for use in unit tests.
///
/// Pre-load it with a sequence of return values:
/// - `Some(text)` — simulates the clipboard containing `text`.
/// - `None`        — simulates an empty clipboard.
///
/// Once the queue is exhausted, `read_text` returns `None` indefinitely.
#[cfg(test)]
pub struct MockReader(pub VecDeque<Option<String>>);

#[cfg(test)]
impl ClipboardReader for MockReader {
    fn read_text(&mut self) -> Option<String> {
        self.0.pop_front().flatten()
    }
}

// ── Poll loop ─────────────────────────────────────────────────────────────────

/// Background thread body: polls, deduplicates, enriches, and emits events.
fn poll_loop(
    mut reader: Box<dyn ClipboardReader>,
    sender: mpsc::Sender<ClipboardEvent>,
    ignore_next: Arc<AtomicBool>,
    super_c_pressed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    display_server: Option<DisplayServer>,
    interval: Duration,
) {
    let mut last_checksum = String::new();

    info!(
        "Clipboard monitor started (poll interval: {}ms)",
        interval.as_millis()
    );

    loop {
        thread::sleep(interval);

        if stop.load(Ordering::Relaxed) {
            info!("Clipboard monitor stopping");
            break;
        }

        // The paste engine sets ignore_next before writing to the clipboard,
        // so the content it just pasted is not re-added to history.
        if ignore_next.swap(false, Ordering::SeqCst) {
            debug!("Skipping clipboard read (ignore_next set)");
            continue;
        }

        let Some(text) = reader.read_text() else {
            continue;
        };

        let checksum = sha256_hex(&text);
        if checksum == last_checksum {
            continue; // Content unchanged.
        }
        last_checksum = checksum;

        // Content changed — resolve the copy source.
        let source = if super_c_pressed.swap(false, Ordering::SeqCst) {
            CopySource::SuperC
        } else {
            CopySource::CtrlC
        };

        // Enrich with the best available MIME type (subprocess, best-effort).
        let (content, mime_type) = enrich_mime(&text, display_server);

        debug!(mime_type, source = source.as_str(), "New clipboard event");

        if sender
            .send(ClipboardEvent {
                content,
                mime_type,
                source,
            })
            .is_err()
        {
            // Receiver was dropped — the daemon is shutting down.
            break;
        }
    }
}

// ── MIME enrichment ───────────────────────────────────────────────────────────

/// MIME type priority — earlier entries are preferred over later ones.
const MIME_PRIORITY: &[&str] = &[
    "text/html",
    "text/uri-list",
    "text/plain;charset=utf-8",
    "text/plain",
];

/// Try to retrieve a richer MIME type for the clipboard content.
///
/// On the first detected change, this function calls `xclip` (X11) or
/// `wl-paste` (Wayland) to list the available MIME types.
///
/// - `text/html` is read and **stripped to plain text** so history always
///   shows readable content rather than raw markup (e.g. Slack copies).
/// - `text/uri-list` is stored as-is.
/// - Everything else falls back to the plain text already read by arboard.
///
/// **Fast path:** if `plain_text` contains no HTML-tag-shaped characters
/// (`<` and `>`) and no URI scheme (`://`), we skip the subprocess calls
/// entirely and return it as plain text.  This avoids spawning `wl-paste`
/// on every plain-text Ctrl+C copy — which caused a taskbar flash on
/// GNOME/Wayland as the short-lived Wayland client briefly registered.
fn enrich_mime(plain_text: &str, display_server: Option<DisplayServer>) -> (String, String) {
    let Some(ds) = display_server else {
        return (plain_text.to_owned(), "text/plain".to_owned());
    };

    // Cheap heuristic — if plain text lacks any tag-shaped or URI-shaped
    // content, skip the subprocess enrichment altogether.
    let looks_like_html = plain_text.contains('<') && plain_text.contains('>');
    let looks_like_uri = plain_text.contains("://");
    if !looks_like_html && !looks_like_uri {
        return (plain_text.to_owned(), "text/plain".to_owned());
    }

    let targets = list_mime_targets(ds);
    if targets.is_empty() {
        return (plain_text.to_owned(), "text/plain".to_owned());
    }

    let best = pick_best_mime(&targets);

    match best.as_str() {
        "text/html" => {
            if let Some(html) = read_mime_content(&best, ds) {
                let clean = strip_html(&html);
                // Only use the stripped version when it's non-empty.
                if !clean.is_empty() {
                    return (clean, "text/plain".to_owned());
                }
            }
        }
        "text/uri-list" => {
            if let Some(uris) = read_mime_content(&best, ds) {
                return (uris, best);
            }
        }
        _ => {}
    }

    (plain_text.to_owned(), "text/plain".to_owned())
}

/// Strip HTML tags and decode common entities, returning plain text.
///
/// Handles the simple subset that Electron/Slack produces:
/// inline tags (`<a>`, `<b>`, `<br>`, etc.) and the five standard entities.
pub(crate) fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut chars = html.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '<' => {
                // Emit a space so text on either side of a stripped tag
                // (e.g. <br>, <p>) doesn't get merged together.
                out.push(' ');
                in_tag = true;
            }
            '>' => in_tag = false,
            '&' if !in_tag => {
                // Collect up to the closing ';' (or give up after 8 chars).
                let mut entity = String::new();
                let mut found_semi = false;
                for _ in 0..8 {
                    match chars.peek() {
                        Some(&';') => {
                            chars.next();
                            found_semi = true;
                            break;
                        }
                        Some(_) => entity.push(chars.next().unwrap()),
                        None => break,
                    }
                }
                if found_semi {
                    match entity.as_str() {
                        "amp" => out.push('&'),
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "nbsp" => out.push(' '),
                        "quot" => out.push('"'),
                        "apos" => out.push('\''),
                        _ => {
                            out.push('&');
                            out.push_str(&entity);
                            out.push(';');
                        }
                    }
                } else {
                    out.push('&');
                    out.push_str(&entity);
                }
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }

    // Collapse runs of whitespace (including \n from <br>) into single spaces.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Select the best MIME type from `targets` according to [`MIME_PRIORITY`].
///
/// Returns `"text/plain"` when none of the priority types are present.
pub(crate) fn pick_best_mime(targets: &[String]) -> String {
    for &priority in MIME_PRIORITY {
        // Use `contains` to match subtypes like `text/plain;charset=utf-8`.
        if targets
            .iter()
            .any(|t| t.contains(priority) || priority.contains(t.as_str()))
        {
            return priority.to_owned();
        }
    }
    "text/plain".to_owned()
}

/// List MIME types available on the clipboard via a platform subprocess.
fn list_mime_targets(ds: DisplayServer) -> Vec<String> {
    match ds {
        DisplayServer::X11 => {
            run_lines("xclip", &["-o", "-selection", "clipboard", "-t", "TARGETS"])
        }
        DisplayServer::Wayland => run_lines("wl-paste", &["--list-types"]),
    }
}

/// Read clipboard content for a specific MIME type via a platform subprocess.
fn read_mime_content(mime: &str, ds: DisplayServer) -> Option<String> {
    let raw = match ds {
        DisplayServer::X11 => run_output("xclip", &["-o", "-selection", "clipboard", "-t", mime]),
        DisplayServer::Wayland => run_output("wl-paste", &["--type", mime]),
    }?;

    // Validate that the output is valid UTF-8 (e.g. HTML may contain latin-1).
    // Fall back to lossy conversion to avoid dropping events on encoding issues.
    Some(
        String::from_utf8(raw)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
    )
}

// ── Subprocess helpers ────────────────────────────────────────────────────────

/// Run `cmd args` and return stdout split into trimmed, non-empty lines.
fn run_lines(cmd: &str, args: &[&str]) -> Vec<String> {
    let Ok(output) = Command::new(cmd).args(args).output() else {
        return vec![];
    };
    if !output.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Run `cmd args` and return stdout bytes on success.
fn run_output(cmd: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(cmd).args(args).output().ok()?;
    output.status.success().then_some(output.stdout)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Lowercase hex SHA-256 of a UTF-8 string — used for deduplication.
pub(crate) fn sha256_hex(input: &str) -> String {
    let hash = Sha256::digest(input.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    // ── SharedClipboard — timing-safe reader for integration tests ────────────

    /// A shared-state reader for timing-sensitive tests.
    ///
    /// Wraps an `Arc<Mutex<Option<String>>>` so tests can mutate clipboard
    /// content independently of poll cycles, avoiding the race conditions
    /// inherent in a pre-loaded `VecDeque`.
    struct SharedClipboard(Arc<Mutex<Option<String>>>);

    impl ClipboardReader for SharedClipboard {
        fn read_text(&mut self) -> Option<String> {
            self.0.lock().unwrap().clone()
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Build a monitor backed by a [`MockReader`] (one-shot VecDeque items).
    ///
    /// Uses a **1 ms** poll interval so tests run fast.  Only suitable for
    /// tests that do **not** need to set flags between polls (race-free use:
    /// flags set before `start_with_reader`, or tested via atomics directly).
    fn make_mock_monitor(
        items: Vec<Option<&str>>,
    ) -> (mpsc::Receiver<ClipboardEvent>, MonitorHandle) {
        let reader = MockReader(items.into_iter().map(|o| o.map(str::to_owned)).collect());
        let config = MonitorConfig {
            poll_interval_ms: 1,
            ..MonitorConfig::default()
        };
        ClipboardMonitor::new(None, &config).start_with_reader(Box::new(reader))
    }

    /// Build a monitor backed by a [`SharedClipboard`].
    ///
    /// Returns the shared state so tests can update the clipboard content
    /// at will.  Uses a **30 ms** poll interval — long enough that calling
    /// `signal_*` immediately after `make_shared_monitor` is guaranteed to
    /// land before the first read.
    fn make_shared_monitor() -> (
        Arc<Mutex<Option<String>>>,
        mpsc::Receiver<ClipboardEvent>,
        MonitorHandle,
    ) {
        let clipboard = Arc::new(Mutex::new(None::<String>));
        let reader = SharedClipboard(Arc::clone(&clipboard));
        let config = MonitorConfig {
            poll_interval_ms: 30,
            ..MonitorConfig::default()
        };
        let (rx, h) = ClipboardMonitor::new(None, &config).start_with_reader(Box::new(reader));
        (clipboard, rx, h)
    }

    /// Collect all events available within `max_ms` milliseconds.
    fn collect_events(rx: &mpsc::Receiver<ClipboardEvent>, max_ms: u64) -> Vec<ClipboardEvent> {
        let deadline = std::time::Instant::now() + Duration::from_millis(max_ms);
        let mut events = Vec::new();
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(e) => events.push(e),
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        events
    }

    // ─────────────────────────────────────────────────────────────────────────
    // strip_html — pure function tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn strip_html_removes_tags() {
        assert_eq!(strip_html("<b>hello</b>"), "hello");
    }

    #[test]
    fn strip_html_slack_email_link() {
        let html = r#"<a href="mailto:user@example.com">National@2026</a>"#;
        assert_eq!(strip_html(html), "National@2026");
    }

    #[test]
    fn strip_html_decodes_entities() {
        assert_eq!(strip_html("a &amp; b &lt;3 &gt; c"), "a & b <3 > c");
        assert_eq!(strip_html("&quot;quoted&quot;"), "\"quoted\"");
        assert_eq!(strip_html("it&apos;s"), "it's");
    }

    #[test]
    fn strip_html_collapses_whitespace() {
        assert_eq!(strip_html("foo  \n  bar"), "foo bar");
    }

    #[test]
    fn strip_html_br_becomes_space() {
        assert_eq!(
            strip_html("line1<br>line2<br><br>line3"),
            "line1 line2 line3"
        );
    }

    #[test]
    fn strip_html_plain_text_unchanged() {
        assert_eq!(strip_html("plain text"), "plain text");
    }

    #[test]
    fn strip_html_empty_input() {
        assert_eq!(strip_html(""), "");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // pick_best_mime — pure function tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn prefers_html_over_plain_text() {
        let targets = vec!["text/plain".to_owned(), "text/html".to_owned()];
        assert_eq!(pick_best_mime(&targets), "text/html");
    }

    #[test]
    fn prefers_uri_list_over_plain_text() {
        let targets = vec!["text/plain".to_owned(), "text/uri-list".to_owned()];
        assert_eq!(pick_best_mime(&targets), "text/uri-list");
    }

    #[test]
    fn prefers_html_over_uri_list() {
        let targets = vec![
            "text/uri-list".to_owned(),
            "text/html".to_owned(),
            "text/plain".to_owned(),
        ];
        assert_eq!(pick_best_mime(&targets), "text/html");
    }

    #[test]
    fn falls_back_to_plain_text_when_nothing_recognised() {
        let targets = vec![
            "image/png".to_owned(),
            "application/octet-stream".to_owned(),
        ];
        assert_eq!(pick_best_mime(&targets), "text/plain");
    }

    #[test]
    fn empty_targets_returns_plain_text() {
        assert_eq!(pick_best_mime(&[]), "text/plain");
    }

    #[test]
    fn matches_subtype_with_charset() {
        let targets = vec!["text/plain;charset=utf-8".to_owned()];
        let best = pick_best_mime(&targets);
        assert!(best.contains("text/plain"), "got: {best}");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // sha256_hex — pure function tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn sha256_hex_is_deterministic() {
        assert_eq!(sha256_hex("hello"), sha256_hex("hello"));
    }

    #[test]
    fn sha256_hex_differs_for_different_input() {
        assert_ne!(sha256_hex("aaa"), sha256_hex("bbb"));
    }

    #[test]
    fn sha256_hex_is_64_chars() {
        assert_eq!(sha256_hex("anything").len(), 64);
    }

    #[test]
    fn sha256_hex_handles_empty_string() {
        assert_eq!(sha256_hex("").len(), 64);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // MockReader — unit tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn mock_reader_returns_items_in_order() {
        let mut reader = MockReader(VecDeque::from([
            Some("first".to_owned()),
            Some("second".to_owned()),
            None,
            Some("third".to_owned()),
        ]));

        assert_eq!(reader.read_text(), Some("first".to_owned()));
        assert_eq!(reader.read_text(), Some("second".to_owned()));
        assert_eq!(reader.read_text(), None);
        assert_eq!(reader.read_text(), Some("third".to_owned()));
        assert_eq!(reader.read_text(), None); // exhausted → None
    }

    // ─────────────────────────────────────────────────────────────────────────
    // poll_loop — event emission (MockReader, deterministic)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn emits_event_when_content_changes() {
        let (rx, _h) = make_mock_monitor(vec![
            Some("first"),
            Some("first"), // consecutive duplicate — deduped
            Some("second"),
        ]);
        let events = collect_events(&rx, 100);
        let content: Vec<&str> = events.iter().map(|e| e.content.as_str()).collect();
        assert!(content.contains(&"first"), "first should be emitted");
        assert!(content.contains(&"second"), "second should be emitted");
    }

    #[test]
    fn deduplicates_consecutive_identical_content() {
        let (rx, _h) = make_mock_monitor(vec![Some("dup"), Some("dup"), Some("dup")]);
        let events = collect_events(&rx, 80);
        assert_eq!(events.len(), 1, "duplicate content must emit only once");
    }

    #[test]
    fn none_values_do_not_emit_events() {
        let (rx, _h) = make_mock_monitor(vec![None, None, None]);
        let events = collect_events(&rx, 60);
        assert!(events.is_empty(), "None reads must not emit events");
    }

    #[test]
    fn ctrl_c_is_the_default_source() {
        let (rx, _h) = make_mock_monitor(vec![Some("ctrl copy")]);
        let events = collect_events(&rx, 80);
        let ev = events
            .iter()
            .find(|e| e.content == "ctrl copy")
            .expect("event must be emitted");
        assert_eq!(ev.source, CopySource::CtrlC);
    }

    #[test]
    fn dropping_handle_stops_the_thread() {
        let (rx, handle) = make_mock_monitor(vec![]);
        drop(handle);
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            rx.recv().is_err(),
            "channel must disconnect after handle drop"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // MonitorHandle flag atomics — tested directly to avoid timing races
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn signal_ignore_next_sets_atomic_flag() {
        // Use a very long poll interval so the thread never reads during the test.
        let reader = MockReader(VecDeque::new());
        let config = MonitorConfig {
            poll_interval_ms: 60_000,
            ..MonitorConfig::default()
        };
        let (_, handle) = ClipboardMonitor::new(None, &config).start_with_reader(Box::new(reader));

        assert!(!handle.ignore_next.load(Ordering::SeqCst), "starts false");
        handle.signal_ignore_next();
        assert!(
            handle.ignore_next.load(Ordering::SeqCst),
            "set after signal"
        );
    }

    #[test]
    fn signal_super_c_sets_atomic_flag() {
        let reader = MockReader(VecDeque::new());
        let config = MonitorConfig {
            poll_interval_ms: 60_000,
            ..MonitorConfig::default()
        };
        let (_, handle) = ClipboardMonitor::new(None, &config).start_with_reader(Box::new(reader));

        assert!(
            !handle.super_c_pressed.load(Ordering::SeqCst),
            "starts false"
        );
        handle.signal_super_c();
        assert!(
            handle.super_c_pressed.load(Ordering::SeqCst),
            "set after signal"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // SharedClipboard — timing-safe integration tests
    //
    // SharedClipboard lets the test mutate clipboard content independently of
    // poll cycles, and uses a 30 ms interval so flag calls made immediately
    // after monitor start are guaranteed to land before the first read.
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn super_c_signal_sets_source_to_super_c() {
        let (clipboard, rx, handle) = make_shared_monitor();

        // Signal super_c and write content — both happen within microseconds,
        // well inside the 30 ms window before the first poll.
        handle.signal_super_c();
        *clipboard.lock().unwrap() = Some("via super+c".to_owned());

        let events = collect_events(&rx, 200);
        let ev = events
            .iter()
            .find(|e| e.content == "via super+c")
            .expect("event for 'via super+c' must be emitted");
        assert_eq!(ev.source, CopySource::SuperC, "source must be SuperC");
    }

    #[test]
    fn ignore_next_causes_poll_to_be_skipped() {
        // Design: clipboard starts with content, ignore_next is set before the
        // first poll, so that poll is skipped entirely (no event emitted).
        // After ignore_next is consumed, the same content is read normally.
        let (clipboard, rx, handle) = make_shared_monitor();

        // Set content and the ignore flag before the first 30 ms poll.
        *clipboard.lock().unwrap() = Some("content".to_owned());
        handle.signal_ignore_next();

        // Wait one poll cycle (≈30 ms) — the cycle should have been skipped.
        std::thread::sleep(Duration::from_millis(35));
        assert!(
            rx.try_recv().is_err(),
            "first poll must be skipped when ignore_next is set"
        );

        // After ignore_next is consumed, the next poll reads normally.
        let events = collect_events(&rx, 200);
        assert!(
            events.iter().any(|e| e.content == "content"),
            "content must appear on the poll after ignore_next clears"
        );
    }

    #[test]
    fn super_c_flag_is_cleared_after_one_event() {
        // The super_c_pressed flag is a one-shot: only the FIRST event after
        // signalling gets SuperC; subsequent events default back to CtrlC.
        let (clipboard, rx, handle) = make_shared_monitor();

        handle.signal_super_c();
        *clipboard.lock().unwrap() = Some("first".to_owned());

        // Wait for "first" event.
        let ev1 = rx
            .recv_timeout(Duration::from_millis(200))
            .expect("first event must arrive");
        assert_eq!(
            ev1.source,
            CopySource::SuperC,
            "first event should be SuperC"
        );

        // Now change clipboard without signalling super_c again.
        *clipboard.lock().unwrap() = Some("second".to_owned());

        let ev2 = rx
            .recv_timeout(Duration::from_millis(200))
            .expect("second event must arrive");
        assert_eq!(
            ev2.source,
            CopySource::CtrlC,
            "second event should be CtrlC"
        );
    }
}
