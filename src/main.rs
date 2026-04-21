//! CopyDeck binary entry point.
//!
//! Parses CLI arguments and dispatches to the appropriate handler.  Heavy
//! logic lives in the library crate (`src/lib.rs` and its modules) so it can
//! be tested independently.

mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Command, PinCommand};
use copydeck::{
    config::Config,
    daemon::CopyDeckDaemon,
    hotkeys::register_gnome_shortcuts,
    ipc::{IpcAction, IpcClient},
    storage::StorageManager,
    utils::deps,
    utils::display::DisplayServer,
};
use serde::{Deserialize, Serialize};
use tracing::info;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialise structured logging.
    // Set COPYDECK_LOG=debug (or trace/warn/error) to override the level.
    let filter = std::env::var("COPYDECK_LOG").unwrap_or_else(|_| "copydeck=info".to_owned());

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    match cli.command {
        Command::Start => cmd_start(),
        Command::Open { paste } => cmd_open(paste),
        Command::Pause => cmd_pause(),
        Command::Resume => cmd_resume(),
        Command::Pin(sub) => cmd_pin(sub),
        Command::CheckDeps => cmd_check_deps(),
        Command::InstallService => cmd_install_service(),
        Command::Config { key, value } => cmd_config(key, value),
    }
}

// ── Command handlers ──────────────────────────────────────────────────────────

/// Start the background daemon.
fn cmd_start() -> Result<()> {
    info!("Starting CopyDeck daemon…");
    let cfg = Config::load()?;
    CopyDeckDaemon::new(cfg)?.run()
}

/// Open the clipboard popup (optionally in paste mode).
fn cmd_open(paste: bool) -> Result<()> {
    info!(paste, "Open popup requested via CLI");
    let action = if paste {
        IpcAction::OpenPaste
    } else {
        IpcAction::Open
    };
    IpcClient::with_default_path().send(action)
}

/// Pause clipboard monitoring.
fn cmd_pause() -> Result<()> {
    info!("Pause monitoring requested");
    IpcClient::with_default_path().send(IpcAction::Pause)
}

/// Resume clipboard monitoring.
fn cmd_resume() -> Result<()> {
    info!("Resume monitoring requested");
    IpcClient::with_default_path().send(IpcAction::Resume)
}

/// Dispatch pin subcommands.
fn cmd_pin(sub: PinCommand) -> Result<()> {
    let cfg = Config::load()?;
    let db = StorageManager::open(&cfg.resolved_db_path())?;

    match sub {
        PinCommand::Add { content, label } => {
            info!(?label, "Pin add");
            let id = db.add_pin(
                &content,
                "text/plain",
                label.as_deref(),
                cfg.general.pin_limit,
            )?;
            let display = label
                .as_deref()
                .unwrap_or_else(|| content.lines().next().unwrap_or(&content));
            println!("Pinned #{id}: {}", truncate(display, 72));
        }

        PinCommand::List => {
            let pins = db.get_pins()?;
            if pins.is_empty() {
                println!("No pinned items.");
            } else {
                println!("{:>4}  {}", "ID", "Label / Content");
                println!("{}", "─".repeat(60));
                for pin in &pins {
                    let display = pin
                        .label
                        .as_deref()
                        .unwrap_or_else(|| pin.content.lines().next().unwrap_or(&pin.content));
                    println!("{:>4}  {}", pin.id, truncate(display, 54));
                }
            }
        }

        PinCommand::Remove { id } => {
            info!(id, "Pin remove");
            if db.remove_pin(id)? {
                println!("Removed pin #{id}.");
            } else {
                eprintln!("No pinned item with id {id}.");
                std::process::exit(1);
            }
        }

        PinCommand::Export { output } => {
            info!(?output, "Pin export");
            let pins = db.get_pins()?;
            let exports: Vec<PinExport> = pins
                .into_iter()
                .map(|p| PinExport {
                    label: p.label,
                    content: p.content,
                    mime_type: p.mime_type,
                })
                .collect();
            let json =
                serde_json::to_string_pretty(&exports).context("serialising pins to JSON")?;

            match output {
                Some(path) => {
                    std::fs::write(&path, &json)
                        .with_context(|| format!("writing {}", path.display()))?;
                    println!("Exported {} pin(s) to {}.", exports.len(), path.display());
                }
                None => println!("{json}"),
            }
        }

        PinCommand::Import { input } => {
            info!(?input, "Pin import");
            let raw = std::fs::read_to_string(&input)
                .with_context(|| format!("reading {}", input.display()))?;
            let items: Vec<PinExport> = serde_json::from_str(&raw)
                .with_context(|| format!("parsing {}", input.display()))?;

            let mut added = 0usize;
            let mut skipped = 0usize;

            for item in &items {
                // Skip exact duplicates (same content + mime_type already pinned).
                let existing = db.get_pins()?;
                let dup = existing
                    .iter()
                    .any(|p| p.content == item.content && p.mime_type == item.mime_type);
                if dup {
                    skipped += 1;
                    continue;
                }
                db.add_pin(
                    &item.content,
                    &item.mime_type,
                    item.label.as_deref(),
                    cfg.general.pin_limit,
                )?;
                added += 1;
            }

            println!(
                "Imported {added} pin(s) from {} ({skipped} duplicate(s) skipped).",
                input.display()
            );
        }
    }
    Ok(())
}

// ── Pin export/import format ───────────────────────────────────────────────────

/// JSON record used by `copydeck pin export` / `copydeck pin import`.
#[derive(Serialize, Deserialize)]
struct PinExport {
    label: Option<String>,
    content: String,
    mime_type: String,
}

/// Check and report all system dependencies.
fn cmd_check_deps() -> Result<()> {
    let statuses = deps::check_all();
    let all_ok = deps::print_status(&statuses);

    println!();

    match DisplayServer::detect() {
        Some(ds) => println!(
            "Display server   : {ds}  (hotkeys: {})",
            if ds.is_x11() {
                "native XGrabKey"
            } else {
                "dconf custom shortcut"
            }
        ),
        None => println!("Display server   : not detected (headless / TTY?)"),
    }

    if !all_ok {
        std::process::exit(1);
    }

    Ok(())
}

/// Install the systemd user service and (on Wayland) register GNOME shortcuts.
fn cmd_install_service() -> Result<()> {
    info!("Installing CopyDeck systemd user service");

    // ── Copy unit file ─────────────────────────────────────────────────────
    let service_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
        .join("systemd")
        .join("user");

    std::fs::create_dir_all(&service_dir)
        .with_context(|| format!("creating {}", service_dir.display()))?;

    let unit_src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("packaging")
        .join("copydeck.service");

    let unit_dst = service_dir.join("copydeck.service");

    if unit_src.exists() {
        std::fs::copy(&unit_src, &unit_dst)
            .with_context(|| format!("copying service file to {}", unit_dst.display()))?;
        println!("Installed service file → {}", unit_dst.display());
    } else {
        // Write the embedded unit file when running from an installed binary.
        std::fs::write(&unit_dst, SYSTEMD_UNIT)
            .with_context(|| format!("writing service file to {}", unit_dst.display()))?;
        println!("Installed service file → {}", unit_dst.display());
    }

    // ── Enable and start via systemctl ────────────────────────────────────
    for args in [
        vec!["--user", "daemon-reload"],
        vec!["--user", "enable", "--now", "copydeck"],
    ] {
        let status = std::process::Command::new("systemctl")
            .args(&args)
            .status()
            .with_context(|| format!("running systemctl {}", args.join(" ")))?;
        if !status.success() {
            eprintln!(
                "Warning: `systemctl {}` exited with {status}",
                args.join(" ")
            );
        }
    }
    println!("CopyDeck service enabled and started.");

    // ── Wayland: register GNOME custom shortcuts ──────────────────────────
    if DisplayServer::detect() == Some(DisplayServer::Wayland) {
        println!("Wayland detected — registering GNOME custom shortcuts…");
        match register_gnome_shortcuts() {
            Ok(()) => println!("GNOME shortcuts registered (Super+C / Super+V)."),
            Err(e) => eprintln!("Could not register GNOME shortcuts: {e}\nSet them manually in Settings → Keyboard → Custom Shortcuts."),
        }
    }

    println!("\nDone. CopyDeck will start automatically on next login.");
    Ok(())
}

/// Embedded systemd unit file (used when the packaging/ directory is absent).
const SYSTEMD_UNIT: &str = include_str!("../packaging/copydeck.service");

/// Get or set a config value.
fn cmd_config(key: Option<String>, value: Option<String>) -> Result<()> {
    match (key, value) {
        (None, _) => {
            // Print the entire config as TOML.
            let cfg = Config::load()?;
            let toml = toml::to_string_pretty(&cfg)?;
            print!("{toml}");
        }
        (Some(k), None) => {
            // Read a single key — implemented in Sprint 8.
            println!("config get {k} — not yet implemented (Sprint 8)");
        }
        (Some(k), Some(v)) => {
            // Write a single key — implemented in Sprint 8.
            println!("config set {k}={v} — not yet implemented (Sprint 8)");
        }
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Truncate `s` to at most `max` characters, appending `…` if truncated.
fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}
