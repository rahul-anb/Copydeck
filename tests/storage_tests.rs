//! Integration tests for the storage layer.
//!
//! Every test uses an in-memory SQLite database so they are:
//!   - Fully isolated (no shared state between tests).
//!   - Filesystem-free (no temp files to clean up).
//!   - Fast (in-memory I/O).
//!
//! Run with:
//!   cargo test --test storage_tests

use copydeck::storage::{CopySource, StorageManager};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a fresh in-memory database.  Panics on any setup error.
fn db() -> StorageManager {
    StorageManager::open_in_memory().expect("open_in_memory failed")
}

/// Insert a plain-text `CtrlC` entry with the default history limit.
fn insert(db: &StorageManager, content: &str) -> Option<i64> {
    db.add_history(content, "text/plain", CopySource::CtrlC, 200)
        .expect("add_history failed")
}

// ── open_in_memory ────────────────────────────────────────────────────────────

#[test]
fn open_creates_empty_history() {
    let db = db();
    let rows = db.get_history(10, 0).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn open_creates_empty_pins() {
    let db = db();
    let rows = db.get_pins().unwrap();
    assert!(rows.is_empty());
}

#[test]
fn open_in_memory_is_idempotent() {
    // Two separate in-memory databases must not share state.
    let a = db();
    let b = db();
    insert(&a, "only in a");
    assert!(b.get_history(10, 0).unwrap().is_empty());
}

// ── add_history ───────────────────────────────────────────────────────────────

#[test]
fn add_history_returns_id_on_insert() {
    let db = db();
    let id = insert(&db, "hello");
    assert!(id.is_some(), "expected an id, got None");
}

#[test]
fn add_history_stores_content_and_mime() {
    let db = db();
    insert(&db, "hello world");
    let rows = db.get_history(10, 0).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, "hello world");
    assert_eq!(rows[0].mime_type, "text/plain");
}

#[test]
fn add_history_stores_source_ctrl_c() {
    let db = db();
    db.add_history("x", "text/plain", CopySource::CtrlC, 200)
        .unwrap();
    let rows = db.get_history(1, 0).unwrap();
    assert_eq!(rows[0].source, CopySource::CtrlC);
}

#[test]
fn add_history_stores_source_super_c() {
    let db = db();
    db.add_history("y", "text/plain", CopySource::SuperC, 200)
        .unwrap();
    let rows = db.get_history(1, 0).unwrap();
    assert_eq!(rows[0].source, CopySource::SuperC);
}

#[test]
fn add_history_deduplicates_consecutive_identical() {
    let db = db();
    let first = insert(&db, "dup");
    let second = insert(&db, "dup");

    assert!(first.is_some(), "first insert should succeed");
    assert!(
        second.is_none(),
        "second identical insert should be a no-op"
    );
    assert_eq!(db.get_history(10, 0).unwrap().len(), 1);
}

#[test]
fn add_history_allows_repeat_after_different_entry() {
    // "aaa" → "bbb" → "aaa"  =  three entries (no dedup on the third).
    let db = db();
    insert(&db, "aaa");
    insert(&db, "bbb");
    let id = insert(&db, "aaa");
    assert!(id.is_some());
    assert_eq!(db.get_history(10, 0).unwrap().len(), 3);
}

#[test]
fn add_history_trims_oldest_beyond_limit() {
    let db = db();
    for i in 0..10 {
        db.add_history(&format!("entry {i}"), "text/plain", CopySource::CtrlC, 5)
            .unwrap();
    }
    let rows = db.get_history(100, 0).unwrap();
    assert_eq!(rows.len(), 5, "history must be trimmed to limit=5");
}

#[test]
fn add_history_preserves_multiline_content() {
    let multiline = "line one\nline two\n  indented\n\ttabbed\n";
    let db = db();
    insert(&db, multiline);
    let rows = db.get_history(1, 0).unwrap();
    assert_eq!(rows[0].content, multiline);
}

#[test]
fn add_history_preserves_unicode() {
    let unicode = "こんにちは 🎉 Ünïcödé";
    let db = db();
    insert(&db, unicode);
    let rows = db.get_history(1, 0).unwrap();
    assert_eq!(rows[0].content, unicode);
}

// ── get_history ───────────────────────────────────────────────────────────────

#[test]
fn get_history_returns_newest_first() {
    let db = db();
    insert(&db, "first");
    insert(&db, "second");
    insert(&db, "third");

    let rows = db.get_history(10, 0).unwrap();
    assert_eq!(rows[0].content, "third");
    assert_eq!(rows[1].content, "second");
    assert_eq!(rows[2].content, "first");
}

#[test]
fn get_history_respects_limit() {
    let db = db();
    for i in 0..5 {
        insert(&db, &format!("e{i}"));
    }
    let rows = db.get_history(3, 0).unwrap();
    assert_eq!(rows.len(), 3);
}

#[test]
fn get_history_pagination_is_consistent() {
    let db = db();
    for i in 0..6 {
        insert(&db, &format!("e{i}"));
    }

    let page1 = db.get_history(3, 0).unwrap();
    let page2 = db.get_history(3, 3).unwrap();

    assert_eq!(page1.len(), 3);
    assert_eq!(page2.len(), 3);

    // Pages must not overlap.
    let ids1: Vec<i64> = page1.iter().map(|r| r.id).collect();
    for row in &page2 {
        assert!(!ids1.contains(&row.id), "pages must not overlap");
    }
}

#[test]
fn get_history_offset_beyond_end_returns_empty() {
    let db = db();
    insert(&db, "one");
    let rows = db.get_history(10, 999).unwrap();
    assert!(rows.is_empty());
}

// ── delete_history ────────────────────────────────────────────────────────────

#[test]
fn delete_history_removes_entry() {
    let db = db();
    insert(&db, "to delete");
    let id = db.get_history(1, 0).unwrap()[0].id;

    assert!(db.delete_history(id).unwrap());
    assert!(db.get_history(10, 0).unwrap().is_empty());
}

#[test]
fn delete_history_returns_false_for_missing_id() {
    let db = db();
    assert!(!db.delete_history(9999).unwrap());
}

// ── clear_history ─────────────────────────────────────────────────────────────

#[test]
fn clear_history_removes_all_entries() {
    let db = db();
    for i in 0..5 {
        insert(&db, &format!("e{i}"));
    }
    let n = db.clear_history().unwrap();
    assert_eq!(n, 5);
    assert!(db.get_history(10, 0).unwrap().is_empty());
}

#[test]
fn clear_history_does_not_affect_pins() {
    let db = db();
    db.add_pin("pinned", "text/plain", Some("label")).unwrap();
    for i in 0..3 {
        insert(&db, &format!("h{i}"));
    }

    db.clear_history().unwrap();

    let pins = db.get_pins().unwrap();
    assert_eq!(pins.len(), 1, "pins must survive clear_history");
}

// ── add_pin ───────────────────────────────────────────────────────────────────

#[test]
fn add_pin_stores_content_and_label() {
    let db = db();
    let id = db.add_pin("SELECT *", "text/plain", Some("SQL")).unwrap();
    assert!(id > 0);

    let pins = db.get_pins().unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].content, "SELECT *");
    assert_eq!(pins[0].label.as_deref(), Some("SQL"));
}

#[test]
fn add_pin_without_label_stores_none() {
    let db = db();
    db.add_pin("no label", "text/plain", None).unwrap();
    let pins = db.get_pins().unwrap();
    assert!(pins[0].label.is_none());
}

#[test]
fn add_pin_assigns_ascending_positions() {
    let db = db();
    db.add_pin("a", "text/plain", None).unwrap();
    db.add_pin("b", "text/plain", None).unwrap();
    db.add_pin("c", "text/plain", None).unwrap();

    let pins = db.get_pins().unwrap();
    assert!(pins[0].position < pins[1].position);
    assert!(pins[1].position < pins[2].position);
}

// ── remove_pin ────────────────────────────────────────────────────────────────

#[test]
fn remove_pin_deletes_item() {
    let db = db();
    let id = db.add_pin("bye", "text/plain", None).unwrap();
    assert!(db.remove_pin(id).unwrap());
    assert!(db.get_pins().unwrap().is_empty());
}

#[test]
fn remove_pin_returns_false_for_missing_id() {
    let db = db();
    assert!(!db.remove_pin(9999).unwrap());
}

// ── update_pin_label ──────────────────────────────────────────────────────────

#[test]
fn update_pin_label_changes_label() {
    let db = db();
    let id = db.add_pin("content", "text/plain", Some("old")).unwrap();
    assert!(db.update_pin_label(id, Some("new")).unwrap());
    assert_eq!(db.get_pins().unwrap()[0].label.as_deref(), Some("new"));
}

#[test]
fn update_pin_label_can_clear_to_none() {
    let db = db();
    let id = db.add_pin("c", "text/plain", Some("had label")).unwrap();
    db.update_pin_label(id, None).unwrap();
    assert!(db.get_pins().unwrap()[0].label.is_none());
}

#[test]
fn update_pin_label_returns_false_for_missing_id() {
    let db = db();
    assert!(!db.update_pin_label(9999, Some("x")).unwrap());
}

// ── reorder_pins ─────────────────────────────────────────────────────────────

#[test]
fn reorder_pins_changes_display_order() {
    let db = db();
    let a = db.add_pin("A", "text/plain", None).unwrap();
    let b = db.add_pin("B", "text/plain", None).unwrap();
    let c = db.add_pin("C", "text/plain", None).unwrap();

    db.reorder_pins(&[c, b, a]).unwrap(); // reverse: C B A

    let pins = db.get_pins().unwrap();
    assert_eq!(pins[0].content, "C");
    assert_eq!(pins[1].content, "B");
    assert_eq!(pins[2].content, "A");
}

// ── get_pins ─────────────────────────────────────────────────────────────────

#[test]
fn get_pins_returns_in_insertion_order_by_default() {
    let db = db();
    db.add_pin("first", "text/plain", None).unwrap();
    db.add_pin("second", "text/plain", None).unwrap();
    db.add_pin("third", "text/plain", None).unwrap();

    let pins = db.get_pins().unwrap();
    assert_eq!(pins[0].content, "first");
    assert_eq!(pins[1].content, "second");
    assert_eq!(pins[2].content, "third");
}

// ── Cross-table invariants ────────────────────────────────────────────────────

#[test]
fn ctrl_c_and_super_c_both_appear_in_history() {
    let db = db();
    db.add_history("via ctrl", "text/plain", CopySource::CtrlC, 200)
        .unwrap();
    db.add_history("via super", "text/plain", CopySource::SuperC, 200)
        .unwrap();

    let rows = db.get_history(10, 0).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "both copy sources must produce history entries"
    );

    // Newest first.
    assert_eq!(rows[0].source, CopySource::SuperC);
    assert_eq!(rows[1].source, CopySource::CtrlC);
}

#[test]
fn history_and_pins_are_independent_tables() {
    let db = db();
    insert(&db, "history item");
    db.add_pin("pin item", "text/plain", Some("P")).unwrap();

    assert_eq!(db.get_history(10, 0).unwrap().len(), 1);
    assert_eq!(db.get_pins().unwrap().len(), 1);

    db.clear_history().unwrap();
    assert_eq!(db.get_pins().unwrap().len(), 1);
}
