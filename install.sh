#!/usr/bin/env bash
# CopyDeck one-liner installer for Debian/Ubuntu.
# Usage: curl -fsSL https://github.com/rahul-anb/Copydeck/releases/latest/download/install.sh | bash
set -euo pipefail

REPO="rahul-anb/Copydeck"

# Detect architecture
ARCH=$(dpkg --print-architecture 2>/dev/null || uname -m)
case "$ARCH" in
    amd64|x86_64)  ARCH="amd64" ;;
    arm64|aarch64) ARCH="arm64" ;;
    *)
        echo "Unsupported architecture: $ARCH" >&2
        exit 1
        ;;
esac

# Find the latest release .deb URL for this arch
echo "Fetching latest CopyDeck release..."
URL=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"browser_download_url"' \
    | grep "_${ARCH}\.deb" \
    | head -1 \
    | cut -d'"' -f4)

if [ -z "$URL" ]; then
    echo "Error: could not find a .deb for $ARCH in the latest release." >&2
    exit 1
fi

TMP=$(mktemp -d)
trap "rm -rf $TMP" EXIT

echo "Downloading $URL ..."
curl -fsSL -o "$TMP/copydeck.deb" "$URL"

echo "Installing package..."
sudo dpkg -i "$TMP/copydeck.deb"
sudo apt-get install -f -y

echo "Registering service and hotkeys..."
copydeck install-service

echo ""
echo "CopyDeck installed. Log out and back in for hotkeys to take effect."
echo "  Super+C  — open clipboard history"
echo "  Super+Shift+V  — open and paste"
