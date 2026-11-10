#!/bin/sh
# Installer for Conduit (macOS & Linux).
#
#   curl -fsSL https://raw.githubusercontent.com/resoul/conduit/main/prod/install.sh | sh
#
set -eu

REPO="${CONDUIT_REPO:-resoul/conduit}"
VERSION="${CONDUIT_VERSION:-latest}"
INSTALL_DIR="${CONDUIT_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

detect_target() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux) os_part="unknown-linux-gnu" ;;
    *) fail "OS $os is not supported by this installer (use install.ps1 on Windows)" ;;
  esac
  case "$arch" in
    arm64 | aarch64) arch_part="aarch64" ;;
    x86_64 | amd64) arch_part="x86_64" ;;
    *) fail "Architecture $arch is not supported (arm64 and x86_64 available)" ;;
  esac
  printf '%s-%s' "$arch_part" "$os_part"
}

download() {
  if have curl; then
    curl -fsSL -o "$2" "$1" || fail "Failed to download $1"
  elif have wget; then
    wget -q -O "$2" "$1" || fail "Failed to download $1"
  else
    fail "curl or wget is required"
  fi
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    fail "sha256sum or shasum is required to verify the download"
  fi
}

target="${CONDUIT_TARGET:-$(detect_target)}"
archive="conduit-$target.tar.gz"

if [ -n "${CONDUIT_BASE_URL:-}" ]; then
  base="$CONDUIT_BASE_URL"
elif [ "$VERSION" = "latest" ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t conduit)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Platform: $target"
say "Downloading $base/$archive..."
download "$base/$archive" "$tmp/$archive"
download "$base/$archive.sha256" "$tmp/$archive.sha256"

expected=$(cut -d' ' -f1 < "$tmp/$archive.sha256")
actual=$(sha256_of "$tmp/$archive")
if [ "$expected" != "$actual" ]; then
  fail "Checksum mismatch (expected $expected, got $actual); installation aborted"
fi
say "Checksum verified successfully."

tar -xzf "$tmp/$archive" -C "$tmp" || fail "Failed to unpack archive"
[ -f "$tmp/conduit" ] || fail "Archive does not contain conduit binary"

mkdir -p "$INSTALL_DIR"
mv "$tmp/conduit" "$INSTALL_DIR/conduit"
chmod +x "$INSTALL_DIR/conduit"

say "Conduit installed successfully to $INSTALL_DIR/conduit!"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    say ""
    say "To use 'conduit' command from your terminal, add it to your PATH:"
    say "  export PATH=\"$INSTALL_DIR:\$PATH\""
    say ""
    say "Add that line to your ~/.zshrc or ~/.bashrc to make it permanent."
    ;;
esac

