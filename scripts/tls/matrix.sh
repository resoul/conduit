#!/bin/sh
# TLS interoperability matrix test against `openssl s_server`.
set -eu

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl not found; skipping matrix"
  exit 0
fi

echo "Running TLS crypto & X.509 unit tests..."
cargo test --lib native::tls

echo "To run live HTTPS smoke tests (requires internet):"
echo "  cargo test --lib native::tls::tests::live_https_tls_smoke_test -- --ignored"
