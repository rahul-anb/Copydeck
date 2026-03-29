//! Display-server detection.
//!
//! Determines at runtime whether the session is running under X11 or Wayland.
//! This affects clipboard access, hotkey registration, and paste injection.

use std::env;
use std::fmt;

/// The display server protocol in use for the current session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    /// X11 / Xorg session (`$DISPLAY` is set).
    X11,
    /// Wayland session (`$WAYLAND_DISPLAY` is set).
    Wayland,
}

impl DisplayServer {
    /// Detect the display server by inspecting environment variables.
    ///
    /// Checks `$WAYLAND_DISPLAY` first (set by all Wayland compositors), then
    /// falls back to `$DISPLAY` (set by X11).  Returns [`None`] when neither
    /// variable is present — e.g. in a headless or pure-TTY environment.
    ///
    /// # Examples
    ///
    /// ```
    /// use copydeck::utils::display::DisplayServer;
    ///
    /// match DisplayServer::detect() {
    ///     Some(DisplayServer::X11)    => println!("Running on X11"),
    ///     Some(DisplayServer::Wayland) => println!("Running on Wayland"),
    ///     None                         => println!("No display server detected"),
    /// }
    /// ```
    pub fn detect() -> Option<Self> {
        if env::var_os("WAYLAND_DISPLAY").is_some() {
            Some(Self::Wayland)
        } else if env::var_os("DISPLAY").is_some() {
            Some(Self::X11)
        } else {
            None
        }
    }

    /// Returns `true` when running under X11.
    pub fn is_x11(self) -> bool {
        self == Self::X11
    }

    /// Returns `true` when running under Wayland.
    pub fn is_wayland(self) -> bool {
        self == Self::Wayland
    }
}

impl fmt::Display for DisplayServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::X11 => write!(f, "X11"),
            Self::Wayland => write!(f, "Wayland"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_server_formats_correctly() {
        assert_eq!(DisplayServer::X11.to_string(), "X11");
        assert_eq!(DisplayServer::Wayland.to_string(), "Wayland");
    }

    #[test]
    fn display_server_predicates() {
        assert!(DisplayServer::X11.is_x11());
        assert!(!DisplayServer::X11.is_wayland());
        assert!(DisplayServer::Wayland.is_wayland());
        assert!(!DisplayServer::Wayland.is_x11());
    }
}
