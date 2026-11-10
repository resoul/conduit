#!/bin/sh
# Package conduit binary into a release archive (.tar.gz) with sha256 checksum.
#
#   scripts/package.sh <target-triple> [binary-path]
#
set -eu

target="${1:?Target platform required, e.g. aarch64-apple-darwin}"
bin="${2:-}"

if [ -z "$bin" ]; then
  if [ -f "target/$target/release/conduit" ]; then
    bin="target/$target/release/conduit"
  elif [ -f "conduit/target/$target/release/conduit" ]; then
    bin="conduit/target/$target/release/conduit"
  elif [ -f "target/release/conduit" ]; then
    bin="target/release/conduit"
  elif [ -f "conduit/target/release/conduit" ]; then
    bin="conduit/target/release/conduit"
  fi
fi

[ -f "$bin" ] || { echo "Error: binary not found ($bin). Run 'cargo build --release -p conduit' first." >&2; exit 1; }

out="dist"
mkdir -p "$out"
stage=$(mktemp -d 2>/dev/null || mktemp -d -t conduit)
trap 'rm -rf "$stage"' EXIT INT TERM

cp "$bin" "$stage/conduit"
chmod 755 "$stage/conduit"

archive="conduit-$target.tar.gz"
tar -C "$stage" -czf "$out/$archive" conduit

cd "$out"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$archive" > "$archive.sha256"
else
  shasum -a 256 "$archive" > "$archive.sha256"
fi

echo "Created release artifact: $out/$archive ($(wc -c < "$archive" | tr -d ' ') bytes)"
cat "$archive.sha256"

