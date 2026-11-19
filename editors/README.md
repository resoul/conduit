# Editor Support for Conduit

Conduit includes a TextMate syntax grammar under `editors/textmate` and a built-in Language Server (`conduit lsp`).

---

## Language Server (`conduit lsp`)

`conduit lsp` speaks LSP over stdio (`Content-Length` JSON-RPC) and provides:
- **Real-time diagnostics** across unsaved buffers and relative imports
- **Hover** with parameter names/types, return unions, `//` doc comments, and `cdt.*` stdlib docs
- **Completion** for `cli.`, `env.`, `json.`, `time.`, `validation.`, user symbols, and keywords

---

## JetBrains IDEs (PhpStorm, RustRover, IntelliJ IDEA, etc.)

### Syntax Highlighting (TextMate Bundle)
1. Open **Settings** / **Preferences** (`Cmd + ,` on macOS or `Ctrl + Alt + S` on Linux/Windows).
2. Navigate to **Editor** → **TextMate Bundles**.
3. Make sure the **TextMate Bundles** plugin is enabled under **Settings** → **Plugins**.
4. Click **+** and select `editors/textmate` from this repository.
5. Click **Apply**.

JetBrains IDEs automatically map the upgraded semantic TextMate scopes in `conduit.tmLanguage.json` (declarations, DSL blocks, HTTP methods, annotations, properties, constants, operators) to your active JetBrains Color Scheme.

### Diagnostics, Hover & Completion (LSP4IJ)
1. Install **LSP4IJ** from JetBrains Marketplace.
2. Open **Settings** → **Languages & Frameworks** → **Language Servers** → **+**.
3. Set **Command** to `conduit lsp` (or `/path/to/conduit lsp`).
4. Under **Mappings**, add **File name pattern**: `*.cdt` (Language Id: `conduit`).
5. Click **Apply**.

---

## Antigravity / Visual Studio Code

1. Open Command Palette (`Cmd + Shift + P` / `Ctrl + Shift + P`).
2. Run **Developer: Install Extension from Location...** and select `editors/textmate`.
3. Run **Developer: Reload Window**.

### JetBrains PhpStorm Dark Colors

You have two ways to enable PhpStorm-style syntax colors:

- **Full theme (`Conduit PhpStorm Dark`)**: Open Command Palette → **Preferences: Color Theme** and choose **Conduit PhpStorm Dark** (`themes/conduit-phpstorm-dark.json`).
- **Keep your current theme (color `.cdt` files only)**: Copy the contents of [`editors/textmate/phpstorm-token-colors.json`](textmate/phpstorm-token-colors.json) into your User `settings.json` (`"editor.tokenColorCustomizations"`). It targets only `*.conduit` scopes without altering other languages.
