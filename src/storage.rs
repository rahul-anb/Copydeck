//! Persistent storage layer.
//!
//! All clipboard history and pinned items are stored in a single SQLite
//! database via [`rusqlite`].  The database file is created automatically on
//! first run and migrated forward as new schema versions are released.
//!
//! # Design decisions
//!
//! - **Bundled SQLite** (`rusqlite` feature `"bundled"`) — the SQLite amalgamation
//!   is compiled into the binary.  No system `libsqlite3` required.
//! - **WAL journal mode** — concurrent reads don't block writes, which matters
//!   when the background monitor thread and the UI thread access the DB at the
//!   same time.
//! - **SHA-256 deduplication** — consecutive identical copies (e.g. pressing
//!   Ctrl+C twice) produce only one history entry.
//! - **Trimming** — old entries beyond `history_limit` are pruned on every
//!   insert, keeping the database small indefinitely.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::Path;

// ── Public types ──────────────────────────────────────────────────────────────

/// How a clipboard entry was created.
///
/// Both `CtrlC` and `SuperC` copies appear in the same history list; this
/// field lets the UI display a small source badge next to each row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopySource {
    /// Standard OS copy — `Ctrl+C` in any application, or any app that writes
    /// to the clipboard directly (terminal selection, browser URL copy, etc.).
    CtrlC,
    /// CopyDeck's own `Super+C` hotkey.
    SuperC,
    /// Written programmatically by an application without a user gesture.
    App,
}

impl CopySource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::CtrlC => "ctrl_c",
            Self::SuperC => "super_c",
            Self::App => "app",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "super_c" => Self::SuperC,
            "app" => Self::App,
            _ => Self::CtrlC,
        }
    }
}

/// A single entry in the clipboard history.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Primary key.
    pub id: i64,
    /// Full clipboard content (UTF-8).
    pub content: String,
    /// MIME type, e.g. `"text/plain"` or `"text/html"`.
    pub mime_type: String,
    /// Unix timestamp (seconds since epoch) when this entry was added.
    pub copied_at: i64,
    /// Lowercase hex SHA-256 of `content` — used internally for deduplication.
    pub checksum: String,
    /// How this entry was copied.
    pub source: CopySource,
}

/// A user-pinned clipboard item that persists across sessions and reboots.
#[derive(Debug, Clone)]
pub struct PinnedItem {
    /// Primary key.
    pub id: i64,
    /// Full clipboard content (UTF-8).
    pub content: String,
    /// MIME type.
    pub mime_type: String,
    /// Optional short display label shown in the popup instead of raw content.
    pub label: Option<String>,
    /// Unix timestamp when the item was pinned.
    pub pinned_at: i64,
    /// Manual sort order; lower values appear first in the popup.
    pub position: i64,
}

// ── StorageManager ────────────────────────────────────────────────────────────

/// Manages all database operations for CopyDeck.
///
/// A single `StorageManager` is created at daemon startup and shared (via
/// `Arc<Mutex<StorageManager>>`) between the clipboard monitor thread and the
/// GTK UI thread.
pub struct StorageManager {
    conn: Connection,
}

impl StorageManager {
    /// Open (or create) the database at `path` and run any pending schema
    /// migrations.
    ///
    /// Intermediate directories are created automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or if a migration fails.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating database directory {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;

        let mut mgr = Self { conn };
        mgr.configure_pragmas()?;
        mgr.migrate()?;
        Ok(mgr)
    }

    /// Open an **in-memory** database.
    ///
    /// The database exists only for the lifetime of the returned
    /// `StorageManager`.  Use this in tests.
    ///
    /// # Examples
    ///
    /// ```
    /// use copydeck::storage::StorageManager;
    ///
    /// let db = StorageManager::open_in_memory().unwrap();
    /// assert!(db.get_history(10, 0).unwrap().is_empty());
    /// ```
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory database")?;
        let mut mgr = Self { conn };
        mgr.configure_pragmas()?;
        mgr.migrate()?;
        Ok(mgr)
    }

    // ── History ───────────────────────────────────────────────────────────────

    /// Add a new entry to clipboard history.
    ///
    /// **Deduplication:** if the most-recent entry already has the same
    /// SHA-256 checksum, the call is a no-op and returns `Ok(None)`.
    ///
    /// **Trimming:** after inserting, entries beyond `limit` are deleted
    /// (oldest first) so the database stays bounded.
    ///
    /// Returns `Ok(Some(id))` on insert, `Ok(None)` when deduplicated.
    pub fn add_history(
        &self,
        content: &str,
        mime_type: &str,
        source: CopySource,
        limit: usize,
    ) -> Result<Option<i64>> {
        let checksum = sha256_hex(content);

        // Skip if the newest entry is identical.
        let latest: Option<String> = self
            .conn
            .query_row(
                "SELECT checksum FROM clipboard_history
                 ORDER BY copied_at DESC, id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();

        if latest.as_deref() == Some(checksum.as_str()) {
            return Ok(None);
        }

        let now = unix_now();
        self.conn.execute(
            "INSERT INTO clipboard_history
                 (content, mime_type, copied_at, checksum, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![content, mime_type, now, checksum, source.as_str()],
        )?;
        let id = self.conn.last_insert_rowid();

        // Trim entries beyond the limit.
        self.conn.execute(
            "DELETE FROM clipboard_history
             WHERE id NOT IN (
                 SELECT id FROM clipboard_history
                 ORDER BY copied_at DESC, id DESC
                 LIMIT ?1
             )",
            params![limit as i64],
        )?;

        Ok(Some(id))
    }

    /// Return a page of history entries, newest first.
    ///
    /// `limit` — maximum number of rows to return.
    /// `offset` — number of rows to skip (0-based).
    pub fn get_history(&self, limit: usize, offset: usize) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, mime_type, copied_at, checksum, source
             FROM clipboard_history
             ORDER BY copied_at DESC, id DESC
             LIMIT ?1 OFFSET ?2",
        )?;

        let mapped = stmt.query_map(params![limit as i64, offset as i64], |row| {
            let source_str: String = row.get(5)?;
            Ok(HistoryEntry {
                id: row.get(0)?,
                content: row.get(1)?,
                mime_type: row.get(2)?,
                copied_at: row.get(3)?,
                checksum: row.get(4)?,
                source: CopySource::from_str(&source_str),
            })
        })?;
        let entries = mapped
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("fetching history entries")?;
        Ok(entries)
    }

    /// Delete a single history entry by ID.
    ///
    /// Returns `true` when a row was deleted, `false` when the ID was not found.
    pub fn delete_history(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM clipboard_history WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Delete **all** history entries and return the number of rows removed.
    ///
    /// Pinned items are not affected.
    pub fn clear_history(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM clipboard_history", [])?)
    }

    // ── Pinned items ──────────────────────────────────────────────────────────

    /// Add a new pinned item and return its assigned ID.
    ///
    /// The new item is appended at the end of the pinned list
    /// (highest `position` value + 1).
    ///
    /// **Rotation:** after inserting, pins beyond `limit` are deleted
    /// (oldest `pinned_at` first) so the pinned list stays bounded.
    pub fn add_pin(
        &self,
        content: &str,
        mime_type: &str,
        label: Option<&str>,
        limit: usize,
    ) -> Result<i64> {
        let now = unix_now();
        let next_pos: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM pinned_items",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        self.conn.execute(
            "INSERT INTO pinned_items (content, mime_type, label, pinned_at, position)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![content, mime_type, label, now, next_pos],
        )?;
        let id = self.conn.last_insert_rowid();

        // Rotate out the oldest pins once the limit is exceeded.
        self.conn.execute(
            "DELETE FROM pinned_items
             WHERE id NOT IN (
                 SELECT id FROM pinned_items
                 ORDER BY pinned_at DESC, id DESC
                 LIMIT ?1
             )",
            params![limit as i64],
        )?;

        Ok(id)
    }

    /// Remove a pinned item by ID.
    ///
    /// Returns `true` when a row was deleted, `false` when the ID was not found.
    pub fn remove_pin(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM pinned_items WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Update the display label of a pinned item.
    ///
    /// Pass `None` to clear the label (the popup then shows raw content).
    /// Returns `true` when a row was updated.
    pub fn update_pin_label(&self, id: i64, label: Option<&str>) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE pinned_items SET label = ?1 WHERE id = ?2",
            params![label, id],
        )?;
        Ok(n > 0)
    }

    /// Reorder pinned items.
    ///
    /// `ids` must list every pinned item ID in the desired display order.
    /// Items not included in `ids` are left at their current position.
    pub fn reorder_pins(&self, ids: &[i64]) -> Result<()> {
        for (pos, &id) in ids.iter().enumerate() {
            self.conn.execute(
                "UPDATE pinned_items SET position = ?1 WHERE id = ?2",
                params![pos as i64, id],
            )?;
        }
        Ok(())
    }

    /// Return all pinned items ordered by `position` (ascending).
    pub fn get_pins(&self) -> Result<Vec<PinnedItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, mime_type, label, pinned_at, position
             FROM pinned_items
             ORDER BY position ASC",
        )?;

        let mapped = stmt.query_map([], |row| {
            Ok(PinnedItem {
                id: row.get(0)?,
                content: row.get(1)?,
                mime_type: row.get(2)?,
                label: row.get(3)?,
                pinned_at: row.get(4)?,
                position: row.get(5)?,
            })
        })?;
        let items = mapped
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("fetching pinned items")?;
        Ok(items)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn configure_pragmas(&self) -> Result<()> {
        self.conn
            .execute_batch(
                // Write-ahead logging: reads don't block writes.
                "PRAGMA journal_mode = WAL;
             -- Enforce referential integrity.
             PRAGMA foreign_keys = ON;
             -- Reasonable cache size (4 MB).
             PRAGMA cache_size = -4000;",
            )
            .context("configuring SQLite pragmas")
    }

    fn migrate(&mut self) -> Result<()> {
        // Bootstrap: the version table may not exist yet.
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);",
        )?;

        let version: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if version < 1 {
            self.migrate_v1()?;
        }

        // Future migrations go here:
        // if version < 2 { self.migrate_v2()?; }

        Ok(())
    }

    /// Initial schema: clipboard history + pinned items tables.
    fn migrate_v1(&mut self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS clipboard_history (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    content     TEXT    NOT NULL,
                    mime_type   TEXT    NOT NULL DEFAULT 'text/plain',
                    copied_at   INTEGER NOT NULL,
                    checksum    TEXT    NOT NULL,
                    source      TEXT    NOT NULL DEFAULT 'ctrl_c'
                );

                -- Fast lookup for the dedup check and newest-first ordering.
                CREATE INDEX IF NOT EXISTS idx_history_time
                    ON clipboard_history (copied_at DESC, id DESC);

                -- Fast existence check by content hash.
                CREATE INDEX IF NOT EXISTS idx_history_checksum
                    ON clipboard_history (checksum);

                CREATE TABLE IF NOT EXISTS pinned_items (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    content     TEXT    NOT NULL,
                    mime_type   TEXT    NOT NULL DEFAULT 'text/plain',
                    label       TEXT,
                    pinned_at   INTEGER NOT NULL,
                    position    INTEGER NOT NULL DEFAULT 0
                );

                INSERT INTO schema_version (version) VALUES (1);",
            )
            .context("applying schema migration v1")
    }
}

// ── Free functions ────────────────────────────────────────────────────────────

/// Compute the lowercase hex SHA-256 digest of a UTF-8 string.
fn sha256_hex(input: &str) -> String {
    let hash = Sha256::digest(input.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Current Unix timestamp in seconds.
fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_secs() as i64
}
