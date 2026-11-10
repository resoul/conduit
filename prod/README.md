# Conduit

A domain-specific programming language for web services, real-time communication, and data streams. Source files use the `.cdt` extension.

## Overview

Conduit treats networking, routing, data schemas, and event channels as language-level primitives rather than external libraries.

- **Zero External Dependencies**: Implemented purely using the Rust standard library (`std`).
- **Lightweight & Self-Contained**: Binary size is under 1 MB, no runtime or compiler needed on the user machine.
- **Strongly Typed**: Static type-checker catches errors before code runs.

## Installation

Rust and Cargo are **not required** to use Conduit.

### macOS & Linux
```bash
curl -fsSL https://raw.githubusercontent.com/resoul/conduit/main/prod/install.sh | sh
```

### Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/resoul/conduit/main/prod/install.ps1 | iex
```

## Usage

```bash
# Type-check a Conduit file
conduit check examples/functions/01_math_and_logic.cdt

# Run a Conduit file
conduit run examples/functions/01_math_and_logic.cdt
```
