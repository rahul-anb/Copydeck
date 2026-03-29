//! GTK4 widget: scrollable clipboard history list.
//!
//! [`HistoryList`] wraps a `GtkListBox` and populates it from a
//! `Vec<HistoryEntry>`.  Each row shows a relative timestamp and a content
//! preview (up to `max_preview_lines` lines).

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Label, ListBox, ListBoxRow, Orientation, ScrolledWindow,
    SelectionMode,
};
use gtk4::pango::EllipsizeMode;

use crate::storage::HistoryEntry;

// ── HistoryList ────────────────────────────────────────────────────────────────

/// A scrollable `GtkListBox` that displays clipboard history entries.
pub struct HistoryList {
    pub scrolled: ScrolledWindow,
    pub list_box: ListBox,
    entries:      Vec<HistoryEntry>,
    max_preview:  usize,
}

impl HistoryList {
    /// Create a new (empty) history list.
    pub fn new(max_preview_lines: usize) -> Self {
        let list_box = ListBox::new();
        list_box.set_selection_mode(SelectionMode::Single);
        list_box.set_activate_on_single_click(false);
        list_box.add_css_class("history-list");

        let scrolled = ScrolledWindow::builder()
            .child(&list_box)
            .vexpand(true)
            .hexpand(true)
            .build();

        Self {
            scrolled,
            list_box,
            entries: Vec::new(),
            max_preview: max_preview_lines,
        }
    }

    /// Replace all rows with `entries` (newest first).
    pub fn populate(&mut self, entries: Vec<HistoryEntry>) {
        // Remove existing rows.
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }
        self.entries = entries;

        for entry in &self.entries {
            let row = build_history_row(entry, self.max_preview);
            // Tag the row with the entry id for retrieval.
            unsafe {
                row.set_data("entry-id", entry.id);
            }
            self.list_box.append(&row);
        }
    }

    /// Filter visible rows to those whose content contains `query`.
    ///
    /// An empty query shows all rows.
    pub fn filter(&self, query: &str) {
        let q = query.to_ascii_lowercase();
        self.list_box.set_filter_func(move |row| {
            if q.is_empty() {
                return true;
            }
            // Widget tree: ListBoxRow → GtkBox (hbox) → {ts_label, content_vbox}
            //              content_vbox → {content_label, overflow_label?}
            let content = row
                .first_child()                          // GtkBox (hbox)
                .and_then(|b| b.first_child())          // timestamp label
                .and_then(|l| l.next_sibling())         // content_vbox (GtkBox)
                .and_then(|v| v.first_child())          // content label (Label)
                .and_downcast::<Label>();

            content
                .map(|l| l.label().to_lowercase().contains(&q))
                .unwrap_or(false)
        });
    }

    /// Return the `HistoryEntry` for the currently selected row, if any.
    pub fn selected_entry(&self) -> Option<&HistoryEntry> {
        let row = self.list_box.selected_row()?;
        let id: i64 = unsafe { *row.data::<i64>("entry-id")?.as_ptr() };
        self.entries.iter().find(|e| e.id == id)
    }

    /// Select the first visible row and scroll to it.
    pub fn select_first(&self) {
        if let Some(row) = self.list_box.row_at_index(0) {
            self.list_box.select_row(Some(&row));
            row.grab_focus();
            self.ensure_row_visible(row);
        }
    }

    /// Move selection up by one row, wrapping to the last row.
    pub fn select_prev(&self) {
        let idx = self
            .list_box
            .selected_row()
            .map(|r| r.index())
            .unwrap_or(0);
        let prev = if idx > 0 { idx - 1 } else { self.entry_count() as i32 - 1 };
        if let Some(row) = self.list_box.row_at_index(prev) {
            self.list_box.select_row(Some(&row));
            row.grab_focus();
            self.ensure_row_visible(row);
        }
    }

    /// Move selection down by one row, wrapping to the first row.
    pub fn select_next(&self) {
        let idx = self
            .list_box
            .selected_row()
            .map(|r| r.index())
            .unwrap_or(-1);
        let next = (idx + 1) % self.entry_count() as i32;
        if let Some(row) = self.list_box.row_at_index(next) {
            self.list_box.select_row(Some(&row));
            row.grab_focus();
            self.ensure_row_visible(row);
        }
    }

    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Schedule a scroll correction for `row` after the current layout/focus-scroll
    /// pass completes.  `allocation().y()` is the ground-truth position written by
    /// GTK's layout manager — same coordinate system as the vadjustment.
    fn ensure_row_visible(&self, row: ListBoxRow) {
        let scrolled = self.scrolled.clone();
        gtk4::glib::idle_add_local(move || {
            let adj  = scrolled.vadjustment();
            let cur  = adj.value();
            let page = adj.page_size();
            if page <= 0.0 { return gtk4::glib::ControlFlow::Break; }

            #[allow(deprecated)]
            let alloc = row.allocation();
            let y     = alloc.y() as f64;
            let row_h = alloc.height() as f64;
            if row_h <= 0.0 { return gtk4::glib::ControlFlow::Break; }

            if y < cur {
                adj.set_value(y.max(adj.lower()));
            } else if y + row_h > cur + page {
                adj.set_value((y + row_h - page).min(adj.upper() - page).max(adj.lower()));
            }
            gtk4::glib::ControlFlow::Break
        });
    }
}

// ── Row builder ───────────────────────────────────────────────────────────────

fn build_history_row(entry: &HistoryEntry, max_lines: usize) -> ListBoxRow {
    let row = ListBoxRow::new();

    let hbox = GtkBox::new(Orientation::Horizontal, 0);
    hbox.add_css_class("row-box");

    // Timestamp
    let ts_label = Label::new(Some(&relative_time(entry.copied_at)));
    ts_label.add_css_class("timestamp-label");
    ts_label.set_halign(gtk4::Align::Start);
    hbox.append(&ts_label);

    // Content preview
    let (preview, overflow) = build_preview(&entry.content, max_lines);
    let content_vbox = GtkBox::new(Orientation::Vertical, 0);

    let content_label = Label::new(Some(&preview));
    content_label.set_halign(gtk4::Align::Start);
    content_label.set_ellipsize(EllipsizeMode::End);
    content_label.set_xalign(0.0);
    content_label.add_css_class("content-label");
    // Full content in tooltip for multiline items.
    if !entry.content.is_empty() {
        content_label.set_tooltip_text(Some(&entry.content));
    }
    content_vbox.append(&content_label);

    if let Some(ov) = overflow {
        let ov_label = Label::new(Some(&ov));
        ov_label.add_css_class("overflow-label");
        ov_label.set_halign(gtk4::Align::Start);
        content_vbox.append(&ov_label);
    }

    hbox.append(&content_vbox);
    row.set_child(Some(&hbox));
    row
}

/// Truncate `content` to `max_lines` lines; return overflow description.
fn build_preview(content: &str, max_lines: usize) -> (String, Option<String>) {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= max_lines {
        return (content.to_owned(), None);
    }
    let preview = lines[..max_lines].join("\n");
    let extra   = lines.len() - max_lines;
    (preview, Some(format!("↵ (+{extra} lines)")))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Format a Unix timestamp as a human-readable relative time string.
pub fn relative_time(unix_ts: i64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(unix_ts);

    let delta = now.saturating_sub(unix_ts);

    match delta {
        0..=59          => "just now".to_owned(),
        60..=3599       => format!("{}m ago", delta / 60),
        3600..=86399    => format!("{}h ago", delta / 3600),
        86400..=172799  => "yesterday".to_owned(),
        _               => {
            // Date only for older entries.
            use std::time::{Duration, UNIX_EPOCH};
            let t = UNIX_EPOCH + Duration::from_secs(unix_ts as u64);
            // Minimal formatting without chrono — "YYYY-MM-DD".
            let secs = unix_ts as u64;
            let days  = secs / 86400;
            let y     = days_to_ymd(days);
            y
        }
    }
}

/// Convert days-since-epoch to a "YYYY-MM-DD" string (no external deps).
fn days_to_ymd(days: u64) -> String {
    // Gregorian calendar computation.
    let z  = days + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y   = yoe + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp  = (5*doy + 2) / 153;
    let d   = doy - (153*mp + 2)/5 + 1;
    let m   = if mp < 10 { mp + 3 } else { mp - 9 };
    let y   = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_preview_short_content() {
        let (p, ov) = build_preview("one line", 3);
        assert_eq!(p, "one line");
        assert!(ov.is_none());
    }

    #[test]
    fn build_preview_truncates_long_content() {
        let content = "a\nb\nc\nd\ne";
        let (p, ov) = build_preview(content, 3);
        assert_eq!(p, "a\nb\nc");
        assert_eq!(ov.as_deref(), Some("↵ (+2 lines)"));
    }

    #[test]
    fn build_preview_exactly_max_lines() {
        let content = "a\nb\nc";
        let (p, ov) = build_preview(content, 3);
        assert_eq!(p, "a\nb\nc");
        assert!(ov.is_none());
    }

    #[test]
    fn relative_time_just_now() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(relative_time(now), "just now");
    }

    #[test]
    fn relative_time_minutes() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(relative_time(now - 120), "2m ago");
    }

    #[test]
    fn relative_time_hours() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(relative_time(now - 7200), "2h ago");
    }

    #[test]
    fn days_to_ymd_epoch() {
        // 1970-01-01 is day 0.
        assert_eq!(days_to_ymd(0), "1970-01-01");
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2024-01-15 = day 19737 since epoch (verified).
        assert_eq!(days_to_ymd(19737), "2024-01-15");
    }
}
