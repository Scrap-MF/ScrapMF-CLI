#!/bin/sh
# scrapmf installer — one-liner:
#   curl -fsSL https://raw.githubusercontent.com/Scrap-MF/ScrapMF-CLI/main/install.sh | sh
#
# Usage: install.sh [VERSION] [INSTALL_DIR]
#   VERSION      release tag ("latest" by default), e.g. v1.0.0
#   INSTALL_DIR  target dir (default: ~/.local/bin)
# Flags:
#   --gnu        download the dynamically-linked gnu build instead of musl
# Environment:
#   SCRAPMF_INSTALL_DIR   same as INSTALL_DIR argument (arg wins)
set -eu

REPO="Scrap-MF/ScrapMF-CLI"
BINARY="scrapmf"
VERSION="${1:-latest}"
case "${1:-}" in
  --gnu) VERSION="${2:-latest}"; LIBC="gnu" ;;
  *) LIBC="musl" ;;
esac
INSTALL_DIR="${SCRAPMF_INSTALL_DIR:-${2:-$HOME/.local/bin}}"

# --- architecture / target detection ---------------------------------------
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64) ARCH_PART="x86_64" ;;
  aarch64|arm64) ARCH_PART="aarch64" ;;
  *)
    echo "✖ unsupported architecture: $ARCH (only x86_64 and aarch64 builds exist)" >&2
    exit 1
    ;;
esac
TARGET="${ARCH_PART}-unknown-linux-${LIBC}"

ASSET="${BINARY}-${TARGET}.tar.gz"
if [ "$VERSION" = "latest" ]; then
  BASE_URL="https://github.com/${REPO}/releases/latest/download"
else
  BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
fi
# Overridable for mirrors/testing: SCRAPMF_RELEASE_URL_BASE
BASE_URL="${SCRAPMF_RELEASE_URL_BASE:-$BASE_URL}"
URL="${BASE_URL}/${ASSET}"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

fetch() {
  # fetch <output> <url>
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$1" "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$1" "$2"
  else
    echo "✖ curl or wget required" >&2
    exit 1
  fi
}

echo "→ Downloading $URL"
# Keep the asset's original filename so the published .sha256 matches.
fetch "$TMPDIR/$ASSET" "$URL"

echo "→ Verifying checksum"
fetch "$TMPDIR/$ASSET.sha256" "${URL}.sha256"
(cd "$TMPDIR" && sha256sum -c "$ASSET.sha256") || {
  echo "✖ checksum verification FAILED — the download is corrupt or tampered with." >&2
  echo "  Aborting. Re-run later or download manually from https://github.com/${REPO}/releases" >&2
  exit 1
}

echo "→ Extracting"
tar xzf "$TMPDIR/$ASSET" -C "$TMPDIR"

BIN_SRC=$(find "$TMPDIR" -name "$BINARY" -type f | head -n1)
if [ -z "$BIN_SRC" ]; then
  echo "✖ binary not found in archive" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
install -Dm755 "$BIN_SRC" "$INSTALL_DIR/$BINARY"
echo "✔ Installed $BINARY to $INSTALL_DIR/$BINARY"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo "⚠ $INSTALL_DIR is not in PATH — add it:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\"   # add to ~/.bashrc or ~/.zshrc"
    ;;
esac

"$INSTALL_DIR/$BINARY" --version
echo "→ Run '$BINARY --help' to get started."
