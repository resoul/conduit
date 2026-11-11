# Editor Support for Conduit

Conduit includes a built-in Language Server Protocol (LSP) server and a TextMate syntax grammar for rich editor support.

---

## JetBrains IDEs (RustRover, IntelliJ IDEA, etc.)

### 1. Syntax Highlighting (TextMate Bundle)
1. Open **Settings** / **Preferences** (`Cmd + ,` on macOS or `Ctrl + Alt + S` on Linux/Windows).
2. Navigate to **Editor** → **TextMate Bundles**.
3. Make sure the **TextMate Bundles** plugin is enabled under **Settings** → **Plugins**.
4. Click the **+** (Add) button and select the `editors/textmate` directory from this repository. This directory contains a `package.json` manifest and the grammar file.
5. Click **Apply**. All `.cdt` files will now have full syntax highlighting.

### 2. Real-Time Diagnostics & LSP (Language Server Protocol)
RustRover does not expose a built-in UI for adding arbitrary LSP servers. Install the **LSP4IJ** plugin from **Settings** → **Plugins**, then restart the IDE.

1. Open **Settings** → **Languages & Frameworks** → **Language Servers** and click **+**.
2. Create a user-defined server with:
   - **Name**: `Conduit LSP`
   - **Command**: `conduit lsp` (or the full path to the binary followed by `lsp`)
   - **File name pattern**: `*.cdt`
3. Save the server and open a `.cdt` file. If `conduit` is not on the IDE's `PATH`, use the full path to the executable, for example `target/debug/conduit lsp` from the project root.

---

## Features Provided by `conduit lsp`

* **Diagnostics on the fly**: Real-time syntax and type error squiggles as you type (`publishDiagnostics`).
* **Hover Documentation**: Hover over keywords (`func`, `type`, `var`) or built-in types and methods (`IndexError`, `len`, `char_count`, `trim`, `lower`, `upper`, `split`, `starts_with`, `ends_with`, `replace`, `append`, `contains`, `get`, `to_string`) to see signatures and usage.
* **Auto-completion**: Triggered automatically or via `Ctrl + Space` and `.` for keywords and built-in methods.

