//! GTK4 widget: pinned items list with drag-to-reorder.
//!
//! [`PinnedList`] wraps a `GtkListBox` and populates it from a
//! `Vec<PinnedItem>`.  Items can be reordered by dragging rows.  Emits a
//! callback whenever the order changes so the storage layer can be updated.

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, DragSource, DropTarget,
    Label, ListBox, ListBoxRow, Orientation, ScrolledWindow, SelectionMode,
};
use gtk4::pango::EllipsizeMode;

use crate::storage::PinnedItem;

// ── PinnedList ────────────────────────────────────────────────────────────────

/// A `GtkListBox` for pinned clipboard items with drag-to-reorder support.
pub struct PinnedList {
    pub scrolled: ScrolledWindow,
    pub list_box: ListBox,
    items:        Vec<PinnedItem>,
    max_preview:  usize,
}

impl PinnedList {
    /// Create a new (empty) pinned list.
    pub fn new(max_preview_lines: usize) -> Self {
        let list_box = ListBox::new();
        list_box.set_selection_mode(SelectionMode::Single);
        list_box.set_activate_on_single_click(false);
        list_box.add_css_class("pinned-list");

        let scrolled = ScrolledWindow::builder()
            .child(&list_box)
            .vexpand(false)
            .hexpand(true)
            .propagate_natural_height(true)
            .build();

        Self { scrolled, list_box, items: Vec::new(), max_preview: max_preview_lines }
    }

    /// Replace all rows with `items` (ordered by `position`).
    pub fn populate(&mut self, items: Vec<PinnedItem>) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }
        self.items = items;

        for item in &self.items {
            let row = build_pinned_row(item, self.max_preview);
            unsafe {
                row.set_data("pin-id", item.id);
            }
            self.attach_drag(row.clone());
            self.list_box.append(&row);
        }
    }

    /// Return the `PinnedItem` for the currently selected row, if any.
    pub fn selected_item(&self) -> Option<&PinnedItem> {
        let row = self.list_box.selected_row()?;
        let id: i64 = unsafe { *row.data::<i64>("pin-id")?.as_ptr() };
        self.items.iter().find(|p| p.id == id)
    }

    /// Return IDs of all visible items in their current display order.
    pub fn ordered_ids(&self) -> Vec<i64> {
        let mut ids = Vec::new();
        let mut row = self.list_box.row_at_index(0);
        while let Some(r) = row {
            if let Some(id) = unsafe { r.data::<i64>("pin-id") } {
                ids.push(unsafe { *id.as_ptr() });
            }
            row = r.next_sibling().and_downcast::<ListBoxRow>();
        }
        ids
    }

    /// Filter visible rows to those whose label/content contains `query`.
    pub fn filter(&self, query: &str) {
        let q = query.to_ascii_lowercase();
        self.list_box.set_filter_func(move |row| {
            if q.is_empty() {
                return true;
            }
            // Widget tree: ListBoxRow → GtkBox (hbox) → content_vbox (GtkBox) → content_label
            let label = row
                .first_child()                  // GtkBox (hbox)
                .and_then(|b| b.first_child())  // content_vbox (GtkBox)
                .and_then(|v| v.first_child())  // content label (Label)
                .and_downcast::<Label>();
            label
                .map(|l| l.label().to_lowercase().contains(&q))
                .unwrap_or(false)
        });
    }

    /// Select the first visible row and scroll to it.
    pub fn select_first(&self) {
        if let Some(row) = self.list_box.row_at_index(0) {
            self.list_box.select_row(Some(&row));
            row.grab_focus();
            self.ensure_row_visible(row);
        }
    }

    /// Select the last visible row and scroll to it.
    pub fn select_last(&self) {
        let count = self.ordered_ids().len() as i32;
        if count > 0 {
            if let Some(row) = self.list_box.row_at_index(count - 1) {
                self.list_box.select_row(Some(&row));
                row.grab_focus();
                self.ensure_row_visible(row);
            }
        }
    }

    /// Select the row whose content matches `content` and scroll to it.
    /// Falls back to the first row if no match is found.
    pub fn select_by_content(&self, content: &str) {
        let mut row_opt = self.list_box.row_at_index(0);
        while let Some(row) = row_opt {
            let matched = unsafe {
                row.data::<i64>("pin-id")
                    .map(|p| *p.as_ptr())
                    .and_then(|id| self.items.iter().find(|i| i.id == id))
                    .map(|item| item.content == content)
                    .unwrap_or(false)
            };
            if matched {
                self.list_box.select_row(Some(&row));
                row.grab_focus();
                self.ensure_row_visible(row);
                return;
            }
            row_opt = row.next_sibling().and_downcast::<ListBoxRow>();
        }
        self.select_first();
    }

    /// Move selection down one row and scroll to it.
    pub fn select_next(&self) {
        let idx = self
            .list_box
            .selected_row()
            .map(|r| r.index())
            .unwrap_or(-1);
        if let Some(row) = self.list_box.row_at_index(idx + 1) {
            self.list_box.select_row(Some(&row));
            row.grab_focus();
            self.ensure_row_visible(row);
        }
    }

    /// Move selection up one row and scroll to it.
    pub fn select_prev(&self) {
        let idx = self
            .list_box
            .selected_row()
            .map(|r| r.index())
            .unwrap_or(1);
        if idx > 0 {
            if let Some(row) = self.list_box.row_at_index(idx - 1) {
                self.list_box.select_row(Some(&row));
                row.grab_focus();
                self.ensure_row_visible(row);
            }
        }
    }

    /// Returns true when the list is empty (no items to display).
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
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

    // ── Drag-to-reorder ───────────────────────────────────────────────────────

    fn attach_drag(&self, row: ListBoxRow) {
        // Drag source — identify the dragged row by its index.
        let drag_source = DragSource::new();
        // Clone row before moving into the prepare closure so we can still
        // call row.add_controller(drag_source) afterwards.
        let row_for_prepare = row.clone();
        drag_source.connect_prepare(move |_, _, _| {
            Some(gtk4::gdk::ContentProvider::for_value(&row_for_prepare.index().to_value()))
        });
        row.add_controller(drag_source);

        // Drop target — reorder the list on drop.
        let list_box = self.list_box.clone();
        let drop_target = DropTarget::new(gtk4::glib::Type::I32, gtk4::gdk::DragAction::MOVE);
        drop_target.connect_drop(move |_target, value, _x, y| {
            let src_idx: i32 = value.get().unwrap_or(-1);
            if src_idx < 0 {
                return false;
            }
            if let Some(dest_row) = list_box.row_at_y(y as i32) {
                let dest_idx = dest_row.index();
                if src_idx != dest_idx {
                    if let Some(src_row) = list_box.row_at_index(src_idx) {
                        list_box.remove(&src_row);
                        list_box.insert(&src_row, dest_idx);
                    }
                }
            }
            true
        });
        row.add_controller(drop_target);
    }
}

// ── Row builder ───────────────────────────────────────────────────────────────

fn build_pinned_row(item: &PinnedItem, max_lines: usize) -> ListBoxRow {
    let row  = ListBoxRow::new();
    let hbox = GtkBox::new(Orientation::Horizontal, 0);
    hbox.add_css_class("row-box");

    let content_vbox = GtkBox::new(Orientation::Vertical, 0);

    // If the item has an explicit label, show it as a single line.
    // Otherwise show the content with multi-line preview like history rows.
    let (preview, overflow) = if let Some(lbl) = item.label.as_deref() {
        (lbl.to_owned(), None)
    } else {
        build_preview(&item.content, max_lines)
    };

    let content_label = Label::new(Some(&preview));
    content_label.set_halign(gtk4::Align::Start);
    content_label.set_hexpand(true);
    content_label.set_ellipsize(EllipsizeMode::End);
    content_label.set_xalign(0.0);
    content_label.add_css_class("content-label");
    content_label.add_css_class("pinned");
    content_label.set_tooltip_text(Some(&item.content));
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

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(id: i64, content: &str, label: Option<&str>) -> PinnedItem {
        PinnedItem {
            id,
            content:   content.to_owned(),
            mime_type: "text/plain".to_owned(),
            label:     label.map(str::to_owned),
            pinned_at: 0,
            position:  id,
        }
    }

    #[test]
    fn pinned_list_created_empty() {
        let pl = PinnedList::new(3);
        assert!(pl.is_empty());
    }

    #[test]
    fn ordered_ids_empty_list() {
        let pl = PinnedList::new(3);
        assert!(pl.ordered_ids().is_empty());
    }

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
    fn build_preview_label_overrides_content() {
        let item = make_item(1, "line1\nline2\nline3\nline4", Some("My Label"));
        let (p, ov) = if let Some(lbl) = item.label.as_deref() {
            (lbl.to_owned(), None)
        } else {
            build_preview(&item.content, 3)
        };
        assert_eq!(p, "My Label");
        assert!(ov.is_none());
    }
}
