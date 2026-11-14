# Editor Setup

Conduit provides syntax highlighting via a TextMate grammar definition located in `editors/textmate`.

> **Note:** The `conduit lsp` daemon from 0.1.x is not yet available in 0.2.0. Syntax highlighting via the TextMate bundle works out of the box.

---

## JetBrains IDEs (RustRover, IntelliJ IDEA)

1. Open **Settings / Preferences** (`Ctrl+Alt+S` or `Cmd+,`).
2. Navigate to **Editor > TextMate Bundles**.
3. Make sure the **TextMate Bundles** plugin is enabled in **Settings > Plugins**.
4. Click the `+` button and add the `conduit/editors/textmate` directory from this repository. It contains a `package.json` manifest and the grammar file.
5. Apply changes. All files ending in `.cdt` will now have syntax highlighting.

---

## Visual Studio Code

Copy `editors/textmate/conduit.tmLanguage.json` to your VS Code syntax directory or extension bundle to register the grammar for `source.conduit`.
