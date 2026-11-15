# Editor Setup

Conduit provides both syntax highlighting (`editors/textmate`) and a built-in Language Server (`conduit lsp`).

---

## Language Server (`conduit lsp`)

Start the stdio JSON-RPC language server with:

```bash
conduit lsp
```

Supported capabilities:
- **Real-time diagnostics**: Type-checks open and unsaved `.cdt` buffers (including multi-file relative imports) on open/change.
- **Hover**: Shows signatures, parameter names/types, return unions, and leading `//` comments for user-defined `func` and `struct` declarations, all `cdt.*` modules (`cdt.cli`, `cdt.env`, `cdt.json`, `cdt.time`, `cdt.validation`), and built-in methods/keywords.
- **Completion**: Dot-completion for `cli.`, `env.`, `json.`, `time.`, and `validation.`, plus user-defined functions, structs, and keywords.

---

## JetBrains IDEs (RustRover, IntelliJ IDEA)

### 1. Syntax Highlighting (TextMate Bundle)
1. Open **Settings / Preferences** (`Ctrl+Alt+S` or `Cmd+,`).
2. Navigate to **Editor > TextMate Bundles**.
3. Make sure the **TextMate Bundles** plugin is enabled in **Settings > Plugins**.
4. Click `+` and select the `conduit/editors/textmate` directory from this repository.
5. Click **Apply**.

### 2. Diagnostics, Hover & Completion (LSP4IJ)
1. Install the **LSP4IJ** plugin from JetBrains Marketplace.
2. Open **Settings > Languages & Frameworks > Language Servers** and click **+**.
3. Set **Command** to `conduit lsp` (or the full path to your `conduit` binary).
4. On the **Mappings** tab, add a **File name pattern**: `*.cdt` (Language Id: `conduit`).
5. Click **Apply**.

---

## Antigravity / Visual Studio Code

1. Open the Command Palette (`Cmd+Shift+P` / `Ctrl+Shift+P`).
2. Run **Developer: Install Extension from Location...** and select `conduit/editors/textmate`.
3. Run **Developer: Reload Window**.
