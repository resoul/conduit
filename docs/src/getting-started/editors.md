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

JetBrains IDEs support Conduit via a TextMate grammar and the LSP4IJ plugin.

### 1. Syntax Highlighting
1. Open **Settings / Preferences** (`Ctrl+Alt+S` or `Cmd+,`).
2. Navigate to **Editor > TextMate Bundles**.
3. Make sure the **TextMate Bundles** plugin is enabled in **Settings > Plugins**.
4. Click the `+` button and add the `conduit/editors/textmate` directory from this repository. It contains a `package.json` manifest and the grammar file.
5. Apply changes. All files ending in `.cdt` will now have accurate syntax highlighting.

### 2. Language Server Protocol (LSP)
1. In **Settings > Plugins**, install **LSP4IJ** and restart the IDE. RustRover does not provide a built-in UI for registering arbitrary LSP servers.
2. Go to **Languages & Frameworks > Language Servers** and click `+` to create a user-defined server:
   - **Name:** `Conduit LSP`
   - **Command:** `conduit lsp` (or the full path to the executable followed by `lsp`)
   - **File name pattern:** `*.cdt`
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

