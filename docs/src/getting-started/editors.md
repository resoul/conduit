# Editor Setup

Conduit provides first-class editor support through a built-in Language Server Protocol (LSP) server and TextMate syntax highlighting definitions.

---

## Capabilities

The Conduit toolchain provides:
- **Real-Time Diagnostics**: Instant feedback for syntax errors, type mismatches, non-exhaustive matches, unhandled errors, and invalid imports.
- **Syntax Highlighting**: Keywords (`func`, `guard`, `switch`, `export`, `import`), custom types, numbers, operators (`|>`, `?`, `??`), and strings.
- **Zero Configuration**: Uses standard JSON-RPC over stdin/stdout.

---

## JetBrains IDEs (RustRover, IntelliJ IDEA)

JetBrains IDEs support Conduit via the built-in LSP client and TextMate bundles:

### 1. Syntax Highlighting
1. Open **Settings / Preferences** (`Ctrl+Alt+S` or `Cmd+,`).
2. Navigate to **Editor > TextMate Bundles**.
3. Click the `+` button and add the `conduit/editors/textmate` directory from this repository.
4. Apply changes. All files ending in `.cdt` will now have accurate syntax highlighting.

### 2. Language Server Protocol (LSP)
1. In **Settings**, go to **Languages & Frameworks > Language Servers**.
2. Click `+` to add a new server configuration:
   - **Name:** `Conduit LSP`
   - **File types:** `*.cdt`
   - **Server type:** `Executable`
   - **Path:** `/path/to/conduit` (or just `conduit` if in your `PATH`)
   - **Arguments:** `lsp`
3. Save and open any `.cdt` file. Type errors and syntax warnings will be highlighted inline as you edit.

---

## Visual Studio Code

1. Install the generic LSP extension (such as **Generic LSP Client** or **LSP Support**).
2. Configure the server command in `settings.json`:

```json
{
  "conduit.lsp.path": "conduit",
  "conduit.lsp.args": ["lsp"]
}
```

3. Copy `editors/textmate/conduit.tmLanguage.json` to your VS Code syntax directory or extension bundle to register the grammar for `source.conduit`.

