//! System dependency checker.
//!
//! Verifies that required system libraries and optional helper binaries are
//! present before CopyDeck attempts to use them.  Prints a formatted status
//! table and actionable `apt install` instructions for anything that is missing.

use std::fmt;
use std::process::Command;

// ── Dependency descriptor ─────────────────────────────────────────────────────

/// How a dependency is detected on the system.
#[derive(Debug)]
enum DepCheck {
    /// Query a shared library through `pkg-config --modversion <pkg>`.
    PkgConfig(&'static str),
    /// Check that an executable exists on `$PATH` via `which`.
    Binary(&'static str),
}

/// A single system dependency that CopyDeck may require at runtime.
#[derive(Debug)]
pub struct Dep {
    /// Human-readable name displayed in the status table.
    pub name: &'static str,
    /// How to test whether the dependency is available.
    check: DepCheck,
    /// If `true`, CopyDeck will refuse to start when this dep is missing.
    pub required: bool,
    /// Suggested `apt` command shown when the dep is absent.
    pub install_hint: &'static str,
}

/// All dependencies checked by [`check_all`].
///
/// Order determines the display order in the status table.
pub static DEPS: &[Dep] = &[
    Dep {
        name: if cfg!(feature = "ui") {
            "libgtk-4  (UI)"
        } else {
            "libgtk-4  (headless build — UI not compiled in)"
        },
        check: DepCheck::PkgConfig("gtk4"),
        // Only required at runtime when compiled with the `ui` feature.
        // A headless build never loads GTK.
        required: cfg!(feature = "ui"),
        install_hint: if cfg!(feature = "ui") {
            "sudo apt install libgtk-4-1"
        } else {
            "rebuild with: cargo build --release --features ui  (needs sudo apt install libgtk-4-dev)"
        },
    },
    Dep {
        name: "xdotool  (X11 paste injection)",
        check: DepCheck::Binary("xdotool"),
        required: false,
        install_hint: "sudo apt install xdotool",
    },
    Dep {
        name: "ydotool  (Wayland paste injection)",
        check: DepCheck::Binary("ydotool"),
        required: false,
        install_hint: "sudo apt install ydotool",
    },
    Dep {
        name: "wl-paste (Wayland clipboard)",
        check: DepCheck::Binary("wl-paste"),
        required: false,
        install_hint: "sudo apt install wl-clipboard",
    },
];

// ── Status result ─────────────────────────────────────────────────────────────

/// The outcome of a single dependency check.
#[derive(Debug)]
pub struct DepStatus<'a> {
    pub dep: &'a Dep,
    pub available: bool,
    /// Version string when discoverable (e.g. from `pkg-config`).
    pub version: Option<String>,
}

impl fmt::Display for DepStatus<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let icon = if self.available { "[OK]" } else { "[--]" };
        let version = self.version.as_deref().unwrap_or("");
        write!(f, "{:<36} {:<6} {}", self.dep.name, icon, version)
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

impl Dep {
    /// Run the check for this dependency and return the result.
    pub fn check(&self) -> DepStatus<'_> {
        match &self.check {
            DepCheck::PkgConfig(pkg) => {
                let out = Command::new("pkg-config")
                    .args(["--modversion", pkg])
                    .output();
                match out {
                    Ok(o) if o.status.success() => DepStatus {
                        dep: self,
                        available: true,
                        version: Some(String::from_utf8_lossy(&o.stdout).trim().to_owned()),
                    },
                    _ => DepStatus {
                        dep: self,
                        available: false,
                        version: None,
                    },
                }
            }
            DepCheck::Binary(bin) => {
                let ok = Command::new("which")
                    .arg(bin)
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                DepStatus {
                    dep: self,
                    available: ok,
                    version: None,
                }
            }
        }
    }
}

/// Check all known dependencies and return their statuses.
pub fn check_all() -> Vec<DepStatus<'static>> {
    DEPS.iter().map(|d| d.check()).collect()
}

/// Print a formatted dependency status table to stdout.
///
/// Returns `true` when all *required* dependencies are available.
pub fn print_status(statuses: &[DepStatus<'_>]) -> bool {
    println!("{:<36} {:<6} {}", "Dependency", "Status", "Version");
    println!("{}", "─".repeat(60));

    let mut all_required_ok = true;

    for s in statuses {
        println!("{s}");
        if !s.available {
            if s.dep.required {
                all_required_ok = false;
                println!("    ✗ Required  → {}", s.dep.install_hint);
            } else {
                println!("    · Optional  → {}", s.dep.install_hint);
            }
        }
    }

    all_required_ok
}
