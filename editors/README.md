# Editor Support for Conduit

Conduit includes a built-in Language Server Protocol (LSP) server and a TextMate syntax grammar for rich editor support.

---

## JetBrains IDEs (RustRover, IntelliJ IDEA, etc.)

### 1. Syntax Highlighting (TextMate Bundle)
1. Open **Settings** / **Preferences** (`Cmd + ,` on macOS or `Ctrl + Alt + S` on Linux/Windows).
2. Navigate to **Editor** → **TextMate Bundles**.
3. Click the **+** (Add) button and select the `editors/textmate` directory from this repository.
4. Click **Apply**. All `.cdt` files will now have full syntax highlighting.

### 2. Real-Time Diagnostics & LSP (Language Server Protocol)
You can connect RustRover to `conduit lsp` using either JetBrains' native LSP support or the LSP plugin:

#### Option A: Native LSP (RustRover / IntelliJ Ultimate)
1. Open **Settings** → **Languages & Frameworks** → **Language Servers**.
2. Click **+** to add a new Language Server:
   - **Name**: `Conduit LSP`
   - **File types**: `*.cdt`
   - **Server command**: `conduit lsp` (or path to your built binary `conduit/target/release/conduit lsp`)
3. Click **OK**.

#### Option B: LSP Support Plugin (Any JetBrains IDE)
1. In **Settings** → **Plugins**, search for **LSP Support** and install it.
2. Restart IDE.
3. In **Settings** → **Languages & Frameworks** → **Language Servers** (or **LSP**):
   - Add mapping: extension `cdt` -> executable `conduit`, arguments `lsp`.

---

## Features Provided by `conduit lsp`

* **Diagnostics on the fly**: Real-time syntax and type error squiggles as you type (`publishDiagnostics`).
* **Hover Documentation**: Hover over keywords (`func`, `type`, `var`) or built-in types and methods (`IndexError`, `len`, `trim`, `lower`, `upper`, `append`, `contains`, `get`) to see signatures and usage.
* **Auto-completion**: Triggered automatically or via `Ctrl + Space` and `.` for keywords and built-in methods.

