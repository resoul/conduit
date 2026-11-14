# Editor Support for Conduit

Conduit includes a TextMate syntax grammar for `.cdt` files under `editors/textmate`.

> **Note:** The built-in `conduit lsp` command from 0.1.x is not yet included in the 0.2.0 bytecode VM rewrite. Syntax highlighting via the TextMate bundle works out of the box.

---

## JetBrains IDEs (RustRover, IntelliJ IDEA, etc.)

### Syntax Highlighting (TextMate Bundle)
1. Open **Settings** / **Preferences** (`Cmd + ,` on macOS or `Ctrl + Alt + S` on Linux/Windows).
2. Navigate to **Editor** → **TextMate Bundles**.
3. Make sure the **TextMate Bundles** plugin is enabled under **Settings** → **Plugins**.
4. Click the **+** (Add) button and select the `editors/textmate` directory from this repository. This directory contains a `package.json` manifest and the grammar file.
5. Click **Apply**. All `.cdt` files will now have full syntax highlighting.

---

## Visual Studio Code

Copy `editors/textmate/conduit.tmLanguage.json` to your VS Code syntax directory or extension bundle to register the grammar for `source.conduit`.
