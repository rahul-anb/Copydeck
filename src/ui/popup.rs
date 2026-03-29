//! GTK4 top-level popup window.
//!
//! [`CopyDeckPopup`] is the main clipboard history popup.  It assembles the
//! search entry, pinned list, and history list into a single frameless
//! `GtkWindow` that appears when the user presses `Super+C` or `Super+V`.
//!
//! # Keyboard shortcuts
//!
//! | Key | Action |
//! |-----|--------|
//! | `↑` / `↓` | Move selection (crosses Pinned ↔ Recent) |
//! | `Enter` | Paste selected item and close |
//! | `Ctrl+Enter` | Paste without closing (multi-paste) |
//! | `p` | Pin / unpin selected item |
//! | `r` | Rename pinned item inline |
//! | `Del` | Delete selected history item |
//! | `Esc` | Close popup |

use std::sync::{Arc, Mutex};

use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, CssProvider, EventControllerKey,
    Label, Orientation, ScrolledWindow, SearchEntry, Separator, StyleContext,
};
use gtk4::glib::{self, clone};

use crate::config::UiConfig;
use crate::paste::{capture_active_window, PasteEngine};
use crate::storage::StorageManager;
use crate::utils::display::DisplayServer;

use super::history_list::HistoryList;
use super::pinned_list::PinnedList;

// ── CopyDeckPopup ─────────────────────────────────────────────────────────────

/// Handle to the popup window.  Clone-safe (internally `Rc`-based by GTK).
#[derive(Clone)]
pub struct CopyDeckPopup {
    window:       ApplicationWindow,
    search:       SearchEntry,
    pinned:       Arc<Mutex<PinnedList>>,
    history:      Arc<Mutex<HistoryList>>,
    db:           Arc<Mutex<StorageManager>>,
    paste_engine: Arc<PasteEngine>,
    ds:           Option<DisplayServer>,
    ui_config:    UiConfig,
    /// Window ID of the app that had focus before the popup opened.
    prev_window:  Arc<Mutex<Option<u64>>>,
    paste_mode:   Arc<Mutex<bool>>,
}

impl CopyDeckPopup {
    /// Build the window.  Call [`show`](Self::show) to make it visible.
    pub fn new(
        app:          &Application,
        db:           Arc<Mutex<StorageManager>>,
        paste_engine: Arc<PasteEngine>,
        ds:           Option<DisplayServer>,
        ui_config:    &UiConfig,
    ) -> Self {
        // Load CSS.
        let css = CssProvider::new();
        css.load_from_data(include_str!("styles.css"));
        StyleContext::add_provider_for_display(
            &gtk4::gdk::Display::default().expect("no display"),
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let window = ApplicationWindow::builder()
            .application(app)
            .title("CopyDeck")
            .default_width(ui_config.popup_width as i32)
            .default_height(ui_config.popup_height as i32)
            .resizable(false)
            .decorated(false)
            .visible(false)
            .build();

        window.add_css_class("copydeck-popup");

        let root = GtkBox::new(Orientation::Vertical, 0);

        // Search entry
        let search_row = GtkBox::new(Orientation::Horizontal, 0);
        search_row.add_css_class("search-entry-row");
        let search = SearchEntry::new();
        search.set_placeholder_text(Some("Search clipboard…"));
        search.set_hexpand(true);
        search_row.append(&search);
        root.append(&search_row);

        root.append(&Separator::new(Orientation::Horizontal));

        // Pinned section
        let pinned_header = Label::new(Some("PINNED"));
        pinned_header.add_css_class("section-header");
        pinned_header.set_halign(gtk4::Align::Start);
        root.append(&pinned_header);

        let pinned  = Arc::new(Mutex::new(PinnedList::new(ui_config.max_preview_lines)));
        {
            let p = pinned.lock().unwrap();
            // Pinned section grows with content but never exceeds 50 % of the window.
            p.scrolled.set_max_content_height((ui_config.popup_height as i32) / 2);
            root.append(&p.scrolled);
        }

        root.append(&Separator::new(Orientation::Horizontal));

        // History section
        let history_header = Label::new(Some("RECENT"));
        history_header.add_css_class("section-header");
        history_header.set_halign(gtk4::Align::Start);
        root.append(&history_header);

        let history = Arc::new(Mutex::new(HistoryList::new(ui_config.max_preview_lines)));
        {
            let h = history.lock().unwrap();
            root.append(&h.scrolled);
        }

        // Keyboard hint bar
        let hint_bar = GtkBox::new(Orientation::Horizontal, 0);
        hint_bar.add_css_class("hint-bar");
        let hint = Label::new(Some("Esc  Enter  ↑↓  Tab=jump  p=pin/unpin  Del"));
        hint.add_css_class("hint-label");
        hint_bar.append(&hint);
        root.append(&hint_bar);

        window.set_child(Some(&root));

        let popup = Self {
            window,
            search,
            pinned,
            history,
            db,
            paste_engine,
            ds,
            ui_config: ui_config.clone(),
            prev_window: Arc::new(Mutex::new(None)),
            paste_mode:  Arc::new(Mutex::new(false)),
        };

        popup.connect_signals();
        popup
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Show the popup and reload data from the database.
    ///
    /// `paste_on_select` — if `true`, the selected item is automatically
    /// pasted into the previous window (Super+V behaviour).
    pub fn show(&self, paste_on_select: bool) {
        // Capture the currently focused window before taking focus away.
        *self.prev_window.lock().unwrap() =
            capture_active_window(self.ds);
        *self.paste_mode.lock().unwrap() = paste_on_select;

        // Hide first so the compositor unmaps the window.  When we call
        // present() below the compositor will place it on the *current*
        // workspace instead of re-activating it on whichever workspace it
        // was last visible on (which would silently switch the user there).
        self.window.set_visible(false);

        self.reload_data();
        self.search.set_text("");

        self.window.present();
        self.search.grab_focus();

        // Queue a second present() via an idle callback so it runs after the
        // GTK main loop has flushed the initial map request to the compositor.
        // This gives GNOME a second chance to raise the window above any
        // surface that grabbed focus between the first present() and the
        // compositor's acknowledgement.
        let win = self.window.clone();
        glib::idle_add_local(move || {
            win.present();
            glib::ControlFlow::Break
        });
    }

    /// Hide the popup and restore focus to the previous window.
    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    // ── Data loading ──────────────────────────────────────────────────────────

    fn reload_data(&self) {
        eprintln!("[copydeck] reload_data: locking db");
        let (pins, history) = {
            let db = self.db.lock().unwrap();
            eprintln!("[copydeck] reload_data: db locked, fetching");
            let pins    = db.get_pins().unwrap_or_default();
            let history = db
                .get_history(self.ui_config.max_preview_lines * 50, 0)
                .unwrap_or_default();
            eprintln!("[copydeck] reload_data: fetched {} pins, {} history", pins.len(), history.len());
            (pins, history)
        };
        eprintln!("[copydeck] reload_data: db released, populating pinned");
        self.pinned.lock().unwrap().populate(pins);
        eprintln!("[copydeck] reload_data: pinned done, populating history");
        self.history.lock().unwrap().populate(history);
        eprintln!("[copydeck] reload_data: done");
    }

    // ── Signal connections ────────────────────────────────────────────────────

    fn connect_signals(&self) {
        // Live search
        let pinned_ref  = Arc::clone(&self.pinned);
        let history_ref = Arc::clone(&self.history);
        self.search.connect_search_changed(move |entry| {
            let q = entry.text().to_string();
            pinned_ref.lock().unwrap().filter(&q);
            history_ref.lock().unwrap().filter(&q);
        });

        // Keyboard navigation
        let popup = self.clone();
        let key_ctrl = EventControllerKey::new();
        key_ctrl.connect_key_pressed(move |_, key, _, mods| {
            popup.handle_key(key, mods)
        });
        self.window.add_controller(key_ctrl);

        // Double-click on history row = paste (close first so focus returns to previous app)
        let popup = self.clone();
        let history_box = self.history.lock().unwrap().list_box.clone();
        history_box.connect_row_activated(move |_, _| {
            popup.paste_selected(true);
        });

        // Double-click on pinned row = paste (close first so focus returns to previous app)
        let popup = self.clone();
        let pinned_box = self.pinned.lock().unwrap().list_box.clone();
        pinned_box.connect_row_activated(move |_, _| {
            popup.paste_selected(true);
        });
    }

    fn handle_key(
        &self,
        key:  gtk4::gdk::Key,
        mods: gtk4::gdk::ModifierType,
    ) -> glib::Propagation {
        use gtk4::gdk::Key as K;
        use gtk4::gdk::ModifierType as Mod;

        // Don't intercept single-character shortcuts when the search entry has
        // focus — the user is typing a query, not issuing commands.
        let searching = self.search.has_focus();

        match key {
            K::Escape => {
                self.hide();
                glib::Propagation::Stop
            }

            K::Return | K::KP_Enter => {
                let multi = mods.contains(Mod::CONTROL_MASK);
                self.paste_selected(!multi); // close unless Ctrl held
                glib::Propagation::Stop
            }

            K::Up => {
                self.move_selection(-1);
                glib::Propagation::Stop
            }

            K::Down => {
                self.move_selection(1);
                glib::Propagation::Stop
            }

            K::Tab => {
                self.jump_section(1);
                glib::Propagation::Stop
            }

            K::ISO_Left_Tab => {
                self.jump_section(-1);
                glib::Propagation::Stop
            }

            K::p if !searching => {
                self.toggle_pin();
                glib::Propagation::Stop
            }

            K::r if !searching => {
                // Inline rename — TODO Sprint 6 polish
                glib::Propagation::Proceed
            }

            K::Delete if !searching => {
                self.delete_selected();
                glib::Propagation::Stop
            }

            _ => glib::Propagation::Proceed,
        }
    }

    // ── Actions ───────────────────────────────────────────────────────────────

    fn paste_selected(&self, close_after: bool) {
        let (content, mime) = self.selected_content();
        let prev = *self.prev_window.lock().unwrap();

        if content.is_empty() {
            return;
        }

        if close_after {
            self.hide();
        }

        let pe = Arc::clone(&self.paste_engine);
        let paste_mode = *self.paste_mode.lock().unwrap();

        if paste_mode || close_after {
            // Run paste in a dedicated OS thread so the GTK main loop is free
            // to flush the window-hide to the Wayland compositor and yield
            // focus back to the previous app before we inject keystrokes.
            // spawn_local(async { blocking_fn() }) blocks the GTK event loop
            // and prevents the compositor from transferring focus in time.
            std::thread::spawn(move || {
                let _ = pe.paste(&content, &mime, prev);
            });
        } else {
            // Just set the clipboard; the user will paste manually.
            let _ = self.paste_engine.set_clipboard(&content, &mime);
        }
    }

    fn selected_content(&self) -> (String, String) {
        // Check pinned first, then history.
        if let Some(item) = self.pinned.lock().unwrap().selected_item() {
            return (item.content.clone(), item.mime_type.clone());
        }
        if let Some(entry) = self.history.lock().unwrap().selected_entry() {
            return (entry.content.clone(), entry.mime_type.clone());
        }
        (String::new(), "text/plain".to_owned())
    }

    fn move_selection(&self, delta: i32) {
        let pinned  = self.pinned.lock().unwrap();
        let history = self.history.lock().unwrap();

        let in_pinned  = pinned.list_box.selected_row().is_some();
        let in_history = !in_pinned && history.list_box.selected_row().is_some();

        if in_pinned {
            if delta < 0 {
                // Move up within pinned; stop at the top row.
                let idx = pinned.list_box.selected_row().map(|r| r.index()).unwrap_or(0);
                if idx > 0 {
                    pinned.select_prev();
                }
            } else {
                // Move down within pinned; at the last row jump to first history row.
                let idx  = pinned.list_box.selected_row().map(|r| r.index()).unwrap_or(0);
                let last = pinned.ordered_ids().len() as i32 - 1;
                if idx >= last {
                    pinned.list_box.unselect_all();
                    history.select_first();
                } else {
                    pinned.select_next();
                }
            }
        } else if in_history {
            if delta < 0 {
                // Move up within history; at the first row jump to last pinned row.
                let idx = history.list_box.selected_row().map(|r| r.index()).unwrap_or(0);
                if idx == 0 && !pinned.is_empty() {
                    history.list_box.unselect_all();
                    pinned.select_last();
                } else {
                    history.select_prev();
                }
            } else {
                history.select_next();
            }
        } else {
            // Nothing selected: Down starts at pinned (if non-empty), else history.
            if delta > 0 {
                if !pinned.is_empty() {
                    pinned.select_first();
                } else {
                    history.select_first();
                }
            }
            // Up with nothing selected: no-op.
        }
    }

    fn toggle_pin(&self) {
        eprintln!("[copydeck] toggle_pin: start");
        let history_entry = { self.history.lock().unwrap().selected_entry().cloned() };
        eprintln!("[copydeck] toggle_pin: history_entry={}", history_entry.is_some());

        if let Some(entry) = history_entry {
            let content = entry.content.clone();
            eprintln!("[copydeck] toggle_pin: calling add_pin");
            let _ = self.db.lock().unwrap().add_pin(&entry.content, &entry.mime_type, None);
            eprintln!("[copydeck] toggle_pin: add_pin done, scheduling idle");
            let popup = self.clone();
            glib::idle_add_local(move || {
                eprintln!("[copydeck] toggle_pin idle: reload_data start");
                popup.reload_data();
                eprintln!("[copydeck] toggle_pin idle: reload_data done, selecting");
                popup.pinned.lock().unwrap().select_by_content(&content);
                eprintln!("[copydeck] toggle_pin idle: done");
                glib::ControlFlow::Break
            });
            eprintln!("[copydeck] toggle_pin: idle scheduled, returning");
            return;
        }

        let pinned_item = { self.pinned.lock().unwrap().selected_item().cloned() };
        eprintln!("[copydeck] toggle_pin: pinned_item={}", pinned_item.is_some());

        if let Some(item) = pinned_item {
            eprintln!("[copydeck] toggle_pin: calling remove_pin");
            let _ = self.db.lock().unwrap().remove_pin(item.id);
            eprintln!("[copydeck] toggle_pin: remove_pin done, scheduling idle");
            let popup = self.clone();
            glib::idle_add_local(move || {
                eprintln!("[copydeck] unpin idle: reload_data start");
                popup.reload_data();
                eprintln!("[copydeck] unpin idle: done");
                glib::ControlFlow::Break
            });
        }
    }

    fn delete_selected(&self) {
        let entry = { self.history.lock().unwrap().selected_entry().cloned() };
        if let Some(entry) = entry {
            let _ = self.db.lock().unwrap().delete_history(entry.id);
            let popup = self.clone();
            glib::idle_add_local(move || {
                popup.reload_data();
                glib::ControlFlow::Break
            });
        }
    }

    /// Jump focus between pinned and history sections.
    ///
    /// `direction > 0` (Tab): pinned → history, history → pinned.
    /// `direction < 0` (Shift+Tab): history → last pinned, pinned → first history.
    fn jump_section(&self, direction: i32) {
        let pinned  = self.pinned.lock().unwrap();
        let history = self.history.lock().unwrap();

        let in_pinned  = pinned.list_box.selected_row().is_some();
        let in_history = !in_pinned && history.list_box.selected_row().is_some();

        if direction > 0 {
            if in_history {
                history.list_box.unselect_all();
                if !pinned.is_empty() { pinned.select_first(); }
            } else {
                pinned.list_box.unselect_all();
                history.select_first();
            }
        } else {
            if in_pinned || (!in_pinned && !in_history) {
                pinned.list_box.unselect_all();
                history.select_first();
            } else {
                history.list_box.unselect_all();
                if !pinned.is_empty() { pinned.select_last(); }
            }
        }
    }
}
