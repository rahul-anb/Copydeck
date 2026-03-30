//! Integration tests for the configuration layer.
//!
//! Tests cover:
//!   - Default values applied for missing keys
//!   - Full round-trip: write TOML → read back → compare
//!   - Invalid TOML returns a descriptive error
//!   - `expand_tilde` path resolution (via `resolved_db_path`)
//!   - Fixture-file round-trip: fixtures/multiline.txt, unicode.txt, etc.
//!
//! Run with:
//!   cargo test --test config_tests

use copydeck::config::{
    Config, GeneralConfig, HotkeyConfig, MonitorConfig, PasteConfig, StorageConfig, UiConfig,
};
use std::path::PathBuf;

// ── Helper ────────────────────────────────────────────────────────────────────

/// Write `contents` to a temp file in `/tmp/` and return its path.
fn write_temp_config(contents: &str) -> PathBuf {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = PathBuf::from(format!("/tmp/copydeck_config_test_{n}.toml"));
    std::fs::File::create(&path)
        .unwrap()
        .write_all(contents.as_bytes())
        .unwrap();
    path
}

// ── Default values ────────────────────────────────────────────────────────────

#[test]
fn default_history_limit_is_200() {
    assert_eq!(Config::default().general.history_limit, 200);
}

#[test]
fn default_content_size_limit_is_512kb() {
    assert_eq!(Config::default().general.content_size_limit_kb, 512);
}

#[test]
fn default_hotkey_open_history_is_super_c() {
    assert_eq!(Config::default().hotkeys.open_history, "super+c");
}

#[test]
fn default_hotkey_open_and_paste_is_super_v() {
    assert_eq!(Config::default().hotkeys.open_and_paste, "super+shift+v");
}

#[test]
fn default_theme_is_auto() {
    assert_eq!(Config::default().ui.theme, "auto");
}

#[test]
fn default_popup_width_is_580() {
    assert_eq!(Config::default().ui.popup_width, 580);
}

#[test]
fn default_max_preview_lines_is_3() {
    assert_eq!(Config::default().ui.max_preview_lines, 3);
}

#[test]
fn default_font_is_monospace_13() {
    assert_eq!(Config::default().ui.font, "Monospace 13");
}

#[test]
fn default_show_timestamps_is_true() {
    assert!(Config::default().ui.show_timestamps);
}

#[test]
fn default_poll_interval_is_500ms() {
    assert_eq!(Config::default().monitor.poll_interval_ms, 500);
}

#[test]
fn default_focus_restore_delay_is_300ms() {
    assert_eq!(Config::default().paste.focus_restore_delay_ms, 300);
}

#[test]
fn default_exclude_apps_contains_keepassxc() {
    let cfg = Config::default();
    assert!(
        cfg.monitor.exclude_apps.iter().any(|s| s == "keepassxc"),
        "keepassxc should be in default exclude_apps"
    );
}

// ── Missing file → defaults ───────────────────────────────────────────────────

#[test]
fn missing_config_file_returns_defaults() {
    let path = PathBuf::from("/tmp/copydeck_config_nonexistent_xyz.toml");
    let _ = std::fs::remove_file(&path);
    let cfg = Config::load_from(&path).unwrap();
    assert_eq!(cfg.general.history_limit, 200);
    assert_eq!(cfg.hotkeys.open_history, "super+c");
}

// ── Partial TOML → missing keys filled with defaults ─────────────────────────

#[test]
fn partial_toml_fills_missing_keys_with_defaults() {
    let path = write_temp_config("[ui]\ntheme = \"dark\"\n");
    let cfg = Config::load_from(&path).unwrap();
    assert_eq!(cfg.ui.theme, "dark", "parsed key must match");
    assert_eq!(
        cfg.general.history_limit, 200,
        "missing key must use default"
    );
    assert_eq!(
        cfg.hotkeys.open_and_paste, "super+shift+v",
        "missing key must use default"
    );
}

#[test]
fn partial_general_section() {
    let path = write_temp_config("[general]\nhistory_limit = 500\n");
    let cfg = Config::load_from(&path).unwrap();
    assert_eq!(cfg.general.history_limit, 500);
    assert_eq!(cfg.general.content_size_limit_kb, 512); // default
}

#[test]
fn partial_monitor_section() {
    let path = write_temp_config("[monitor]\npoll_interval_ms = 250\n");
    let cfg = Config::load_from(&path).unwrap();
    assert_eq!(cfg.monitor.poll_interval_ms, 250);
    // exclude_apps must still have the defaults
    assert!(!cfg.monitor.exclude_apps.is_empty());
}

// ── Full round-trip ───────────────────────────────────────────────────────────

#[test]
fn full_round_trip_through_toml() {
    let mut cfg = Config::default();
    cfg.general.history_limit = 123;
    cfg.hotkeys.open_history = "ctrl+alt+c".to_owned();
    cfg.ui.theme = "dark".to_owned();
    cfg.monitor.poll_interval_ms = 100;
    cfg.paste.focus_restore_delay_ms = 50;

    let toml = toml::to_string_pretty(&cfg).unwrap();
    let back: Config = toml::from_str(&toml).unwrap();

    assert_eq!(back.general.history_limit, 123);
    assert_eq!(back.hotkeys.open_history, "ctrl+alt+c");
    assert_eq!(back.ui.theme, "dark");
    assert_eq!(back.monitor.poll_interval_ms, 100);
    assert_eq!(back.paste.focus_restore_delay_ms, 50);
}

// ── Invalid TOML → error ──────────────────────────────────────────────────────

#[test]
fn invalid_toml_returns_error() {
    let path = write_temp_config("this is not valid toml ][[\n");
    let result = Config::load_from(&path);
    assert!(result.is_err(), "invalid TOML must return an error");
}

#[test]
fn wrong_type_for_history_limit_returns_error() {
    let path = write_temp_config("[general]\nhistory_limit = \"not a number\"\n");
    let result = Config::load_from(&path);
    assert!(result.is_err(), "wrong type must return an error");
}

// ── db_path resolution ────────────────────────────────────────────────────────

#[test]
fn resolved_db_path_expands_tilde() {
    let cfg = Config::default();
    let p = cfg.resolved_db_path();
    let s = p.to_string_lossy();
    assert!(
        !s.starts_with('~'),
        "resolved_db_path must not contain a leading tilde; got: {s}"
    );
    assert!(
        s.ends_with("copydeck.db"),
        "resolved_db_path must end with 'copydeck.db'; got: {s}"
    );
}

#[test]
fn resolved_db_path_with_explicit_absolute_path() {
    let path = write_temp_config("[storage]\ndb_path = \"/tmp/test_copydeck.db\"\n");
    let cfg = Config::load_from(&path).unwrap();
    assert_eq!(
        cfg.resolved_db_path(),
        PathBuf::from("/tmp/test_copydeck.db")
    );
}

// ── Fixture file round-trip through storage ───────────────────────────────────

/// Load a fixture file, store it in the DB, and verify it round-trips intact.
fn fixture_round_trip(file: &str) {
    let content = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(file),
    )
    .unwrap_or_else(|_| panic!("fixture {file} not found"));

    let db = copydeck::storage::StorageManager::open_in_memory().unwrap();
    db.add_history(
        &content,
        "text/plain",
        copydeck::storage::CopySource::CtrlC,
        200,
    )
    .unwrap();
    let rows = db.get_history(1, 0).unwrap();
    assert_eq!(
        rows[0].content, content,
        "fixture {file} must round-trip unchanged"
    );
}

#[test]
fn multiline_fixture_round_trips() {
    fixture_round_trip("multiline.txt");
}

#[test]
fn unicode_fixture_round_trips() {
    fixture_round_trip("unicode.txt");
}

#[test]
fn html_fixture_round_trips() {
    fixture_round_trip("rich.html");
}

#[test]
fn uri_list_fixture_round_trips() {
    fixture_round_trip("uris.txt");
}
