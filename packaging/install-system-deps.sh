#!/usr/bin/env bash
# install-system-deps.sh
#
# Install all system libraries and helper binaries that CopyDeck needs at
# runtime on Ubuntu 22.04 or later.
#
# Usage:
#   sudo bash packaging/install-system-deps.sh
#
# What gets installed:
#   libgtk-4-1        — GTK4 runtime (UI)
#   libglib2.0-0      — GLib runtime (GTK dependency)
#   xdotool           — X11 keystroke injection (paste on X11)
#   ydotool           — Wayland keystroke injection (paste on Wayland)
#   wl-clipboard      — wl-copy / wl-paste (Wayland clipboard access)
#
# Note: libgtk-4-1 and libglib2.0-0 are pre-installed on Ubuntu 22.04+
# desktop environments.  This script makes installation explicit and
# repeatable for CI, VMs, and minimal installs.

set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "error: this script must be run as root (use sudo)" >&2
    exit 1
fi

apt-get update -qq

apt-get install -y \
    libgtk-4-1      \
    libglib2.0-0    \
    xdotool         \
    ydotool         \
    wl-clipboard

echo ""
echo "All CopyDeck system dependencies installed successfully."
echo "Run 'copydeck check-deps' to verify."
