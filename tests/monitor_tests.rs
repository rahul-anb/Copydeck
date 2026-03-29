//! Integration tests for the clipboard monitor.
//!
//! All tests use a [`MockReader`] or [`SharedClipboard`] so no real display
//! server is required.  The monitor's background thread runs at 1–30 ms poll
//! intervals to keep the suite fast.
//!
//! Run with:
//!   cargo test --test monitor_tests

use copydeck::{
    config::MonitorConfig,
    monitor::{ClipboardMonitor, ClipboardEvent, ClipboardReader},
    storage::CopySource,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

// ── Local test helpers ────────────────────────────────────────────────────────

/// Deterministic reader backed by a pre-loaded VecDeque.
struct LocalMockReader(VecDeque<Option<String>>);

impl ClipboardReader for LocalMockReader {
    fn read_text(&mut self) -> Option<String> {
        self.0.pop_front().flatten()
    }
}

/// Build a monitor backed by a `LocalMockReader` at 1 ms poll interval.
fn mock_monitor(items: Vec<Option<&str>>)
-> (std::sync::mpsc::Receiver<ClipboardEvent>, copydeck::monitor::MonitorHandle)
{
    let reader = LocalMockReader(
        items.into_iter()
            .map(|o| o.map(str::to_owned))
            .collect(),
    );
    let cfg = MonitorConfig { poll_interval_ms: 1, ..MonitorConfig::default() };
    ClipboardMonitor::new(None, &cfg).start_with_reader(Box::new(reader))
}

/// Collect all events available within `max_ms` milliseconds.
fn collect(rx: &std::sync::mpsc::Receiver<ClipboardEvent>, max_ms: u64) -> Vec<ClipboardEvent> {
    let deadline = std::time::Instant::now() + Duration::from_millis(max_ms);
    let mut events = Vec::new();
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(e)  => events.push(e),
            Err(_) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    events
}

// ── Deduplication ─────────────────────────────────────────────────────────────

#[test]
fn consecutive_duplicates_emit_one_event() {
    let (rx, _h) = mock_monitor(vec![
        Some("dup"), Some("dup"), Some("dup"),
    ]);
    let events = collect(&rx, 80);
    assert_eq!(events.len(), 1, "consecutive duplicates must emit exactly one event");
}

#[test]
fn non_consecutive_duplicates_emit_two_events() {
    // "a" → "b" → "a" : all three are distinct consecutive pairs, so 3 events.
    let (rx, _h) = mock_monitor(vec![
        Some("a"), Some("b"), Some("a"),
    ]);
    let events = collect(&rx, 80);
    assert_eq!(events.len(), 3, "non-consecutive duplicates must each emit an event");
}

// ── Source attribution ────────────────────────────────────────────────────────

#[test]
fn passive_copy_is_attributed_ctrl_c() {
    let (rx, _h) = mock_monitor(vec![Some("hello")]);
    let events   = collect(&rx, 80);
    let ev = events.iter().find(|e| e.content == "hello").expect("event must arrive");
    assert_eq!(ev.source, CopySource::CtrlC, "passive copy must be CtrlC");
}

// ── MIME type ─────────────────────────────────────────────────────────────────

#[test]
fn event_mime_type_defaults_to_text_plain_in_headless_mode() {
    // Without a display server, enrich_mime falls back to text/plain.
    let (rx, _h) = mock_monitor(vec![Some("some text")]);
    let events   = collect(&rx, 80);
    assert!(!events.is_empty(), "event must be emitted");
    assert_eq!(events[0].mime_type, "text/plain");
}

// ── Monitor handle flags ──────────────────────────────────────────────────────

#[test]
fn ignore_next_suppresses_one_event() {
    // Set the ignore flag before the first poll; the event must be skipped.
    // After the flag clears, the same content is read normally.
    use std::sync::atomic::Ordering;
    let clipboard = Arc::new(Mutex::new(None::<String>));
    let reader    = SharedClipboard(Arc::clone(&clipboard));
    let cfg       = MonitorConfig { poll_interval_ms: 30, ..MonitorConfig::default() };
    let (rx, handle) = ClipboardMonitor::new(None, &cfg).start_with_reader(Box::new(reader));

    *clipboard.lock().unwrap() = Some("secret".to_owned());
    handle.ignore_next.store(true, Ordering::SeqCst);

    // Wait one poll — must be skipped.
    std::thread::sleep(Duration::from_millis(35));
    assert!(rx.try_recv().is_err(), "ignored poll must not emit");

    // Second poll — now emitted.
    let events = collect(&rx, 200);
    assert!(events.iter().any(|e| e.content == "secret"), "second poll must emit");
}

#[test]
fn super_c_flag_attributes_source_correctly() {
    use std::sync::atomic::Ordering;
    let clipboard = Arc::new(Mutex::new(None::<String>));
    let reader    = SharedClipboard(Arc::clone(&clipboard));
    let cfg       = MonitorConfig { poll_interval_ms: 30, ..MonitorConfig::default() };
    let (rx, handle) = ClipboardMonitor::new(None, &cfg).start_with_reader(Box::new(reader));

    handle.super_c_pressed.store(true, Ordering::SeqCst);
    *clipboard.lock().unwrap() = Some("via super+c".to_owned());

    let events = collect(&rx, 200);
    let ev = events.iter().find(|e| e.content == "via super+c")
        .expect("event must arrive");
    assert_eq!(ev.source, CopySource::SuperC, "event source must be SuperC");
}

// ── Thread lifecycle ──────────────────────────────────────────────────────────

#[test]
fn dropping_handle_disconnects_channel() {
    let (rx, handle) = mock_monitor(vec![]);
    drop(handle);
    std::thread::sleep(Duration::from_millis(20));
    assert!(rx.recv().is_err(), "channel must close after handle drop");
}

// ── Multiline and unicode content ─────────────────────────────────────────────

#[test]
fn multiline_content_preserved() {
    let content = include_str!("fixtures/multiline.txt");
    let (rx, _h) = mock_monitor(vec![Some(content)]);
    let events   = collect(&rx, 80);
    assert!(!events.is_empty(), "must emit event for multiline content");
    assert_eq!(events[0].content, content, "content must be preserved exactly");
}

#[test]
fn unicode_content_preserved() {
    let content = include_str!("fixtures/unicode.txt");
    let (rx, _h) = mock_monitor(vec![Some(content)]);
    let events   = collect(&rx, 80);
    assert!(!events.is_empty(), "must emit event for unicode content");
    assert_eq!(events[0].content, content, "unicode content must be preserved");
}

// ── SharedClipboard helper ─────────────────────────────────────────────────────

/// A shared-state reader for timing-sensitive tests.
///
/// Wraps `Arc<Mutex<Option<String>>>` so the test can update the clipboard
/// content independently of poll cycles.
struct SharedClipboard(Arc<Mutex<Option<String>>>);

impl copydeck::monitor::ClipboardReader for SharedClipboard {
    fn read_text(&mut self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}
