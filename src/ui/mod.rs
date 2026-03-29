//! GTK4 popup UI.
//!
//! This module is only compiled when the `ui` feature is enabled:
//! ```sh
//! cargo build --features ui
//! ```
//!
//! # Module layout
//!
//! | Module | Widget |
//! |--------|--------|
//! | [`popup`] | Top-level `GtkWindow` — assembles all sections |
//! | [`history_list`] | `GtkListBox` for recent clipboard entries |
//! | [`pinned_list`] | `GtkListBox` for pinned items with drag-to-reorder |
//!
//! CSS is embedded at compile time from `styles.css`.

pub mod history_list;
pub mod pinned_list;
pub mod popup;
