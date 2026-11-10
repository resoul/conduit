# Installation

Conduit can be installed as a standalone pre-built executable or compiled directly from source. Rust and Cargo are **not required** if you install via the pre-built installer scripts.

---

## Pre-Built Executables

Pre-compiled binary releases are available for macOS, Linux, and Windows.

### macOS & Linux

Install the latest release using the automated shell script:

```bash
curl -fsSL https://raw.githubusercontent.com/resoul/conduit/main/install.sh | sh
```

The script downloads the appropriate binary for your system architecture (e.g., `x86_64` or `aarch64`), verifies the archive, and installs `conduit` into `~/.local/bin` (or `/usr/local/bin` if elevated permissions are available).

### Windows (PowerShell)

Install via PowerShell:

```powershell
irm https://raw.githubusercontent.com/resoul/conduit/main/install.ps1 | iex
```

The installer places `conduit.exe` into your user binaries directory and updates your environment `PATH`.

---

## Building from Source

If you prefer building from source or are contributing to Conduit, you need the stable Rust toolchain (1.75+):

```bash
# 1. Clone the repository
git clone https://github.com/resoul/conduit.git
cd conduit

# 2. Build the optimized release binary
cargo build --release

# 3. Optional: Copy binary to a directory in your PATH
cp target/release/conduit ~/.local/bin/
```

Because Conduit contains **zero external crate dependencies**, building from source completes in just a few seconds without downloading any external packages from `crates.io`.

---

## Verifying the Installation

Verify that the CLI is accessible and prints the correct version:

```bash
conduit --version
```

Expected output:

```text
Conduit v0.1.4
```

