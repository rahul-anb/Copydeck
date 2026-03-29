//! Command-line interface definitions.
//!
//! All subcommands and flags are declared here using `clap` derive macros.
//! The actual dispatch logic lives in `main.rs` to keep this file focused on
//! the interface contract.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// CopyDeck — a lightweight clipboard manager for Linux.
///
/// Copies made with Ctrl+C, Super+C, or any application are stored in a
/// scrollable history list.  Use Super+V to open the popup and paste any
/// previous entry.  Pinned items persist across reboots.
#[derive(Parser, Debug)]
#[command(
    name = "copydeck",
    version,
    author,
    about,
    propagate_version = true,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start the CopyDeck background daemon.
    ///
    /// The daemon monitors the clipboard, registers global hotkeys, and serves
    /// the GTK4 popup UI.  It is normally started automatically by the systemd
    /// user service installed with `copydeck install-service`.
    Start,

    /// Open the clipboard history popup.
    ///
    /// When the daemon is running, this sends an IPC signal to open the popup
    /// in the context of the currently active window.  When no daemon is
    /// running it exits with an error.
    Open {
        /// Immediately paste the selected item into the previously focused
        /// window (equivalent to pressing Enter in the popup).
        #[arg(long, short)]
        paste: bool,
    },

    /// Pause clipboard monitoring.
    ///
    /// While paused, copies are not added to history.  Use this before
    /// entering passwords or other sensitive content.  Resume with
    /// `copydeck resume`.
    Pause,

    /// Resume clipboard monitoring after `copydeck pause`.
    Resume,

    /// Manage pinned clipboard items.
    ///
    /// Pinned items persist across sessions and reboots.  They appear at the
    /// top of the popup above the rolling history.
    #[command(subcommand)]
    Pin(PinCommand),

    /// Print the status of all system dependencies and exit.
    ///
    /// Exits with code 0 when all required dependencies are present,
    /// or code 1 when any required dependency is missing.
    CheckDeps,

    /// Install and enable the systemd user service.
    ///
    /// Copies the unit file to `~/.config/systemd/user/` and runs
    /// `systemctl --user enable --now copydeck`.  On Wayland, also registers
    /// `Super+C` / `Super+V` as GNOME custom keyboard shortcuts via gsettings.
    InstallService,

    /// Get or set a configuration value without opening the config file.
    ///
    /// # Examples
    ///
    /// ```text
    /// copydeck config                      # print entire config as TOML
    /// copydeck config ui.theme             # read a single key
    /// copydeck config ui.theme dark        # set a key
    /// ```
    Config {
        /// Dotted key path, e.g. `ui.theme` or `general.history_limit`.
        /// Omit to print the entire configuration.
        key: Option<String>,

        /// New value to write.  Omit to read the current value.
        value: Option<String>,
    },
}

/// Subcommands for [`Command::Pin`].
#[derive(Subcommand, Debug)]
pub enum PinCommand {
    /// Add a new pinned item.
    ///
    /// # Examples
    ///
    /// ```text
    /// copydeck pin add "SELECT * FROM users LIMIT 10" --label "Quick SQL"
    /// ```
    Add {
        /// The text content to pin.
        content: String,

        /// Short display label shown in the popup instead of raw content.
        #[arg(long, short)]
        label: Option<String>,
    },

    /// List all pinned items with their IDs and labels.
    List,

    /// Remove a pinned item by its numeric ID.
    ///
    /// Use `copydeck pin list` to find the ID.
    Remove {
        /// Numeric ID of the item to remove.
        id: i64,
    },

    /// Export all pinned items to a JSON file (or stdout).
    ///
    /// The exported format can be imported on another machine with
    /// `copydeck pin import`.
    Export {
        /// Destination file.  Writes to stdout if omitted.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },

    /// Import pinned items from a JSON file created with `copydeck pin export`.
    ///
    /// Duplicate items (same content + MIME type) are silently skipped.
    Import {
        /// Path to the JSON export file.
        input: PathBuf,
    },
}
