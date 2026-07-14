# Developing the Pyfun VS Code extension

The client is deliberately thin: it only launches `pyfun lsp` and wires up document sync. Every
feature lives in the Rust server (`src/lsp/`). Doc-comment hover (`##` doc comments rendered below
the type) landed 2026-07-03. Truly incremental reparse was decided against: whole-file analysis is
milliseconds at this scale and the server's fingerprint-validated caches already remove redundant
re-analysis (see `ROADMAP.md`).

## Build and install a `.vsix` (to just use a local build)

If you only want the extension working in your editor, not to hack on the client, build a `.vsix`
and install it. This is more reliable than the F5 dev-host flow below.

```bash
cargo build                      # produces target/debug/pyfun(.exe) — the server
cd editors/vscode
npm install
npx @vscode/vsce package         # produces pyfun-<version>.vsix
```

Then install with the CLI of the VS Code variant you actually run — use `code-insiders` for
Insiders, `code` for stable. **Use the `bin/code(.cmd)` CLI wrapper, not the GUI `code.exe`** (the
latter just launches the app instead of installing). A raw folder copy into the extensions directory
does **not** work: VS Code only loads extensions registered in its `extensions/extensions.json`
manifest, which `--install-extension` updates.

```bash
code-insiders --install-extension pyfun-0.1.0.vsix --force
```

Point the extension at the built server via a setting (see below), then reload the window. An
already-running window won't pick up a newly installed extension until **Developer: Reload Window**.

## Setup (development)

The extension shells out to a `pyfun` executable. From the repo root:

```bash
cargo build                      # produces target/debug/pyfun
```

Then either put `pyfun` on your `PATH`, or set `pyfun.server.path` in VS Code settings to the built
binary. `${workspaceFolder}` is expanded by the client, so a checkout-relative path works on any
machine:

```jsonc
{
  "pyfun.server.path": "${workspaceFolder}/target/debug/pyfun.exe"
}
```

Install the client dependency:

```bash
cd editors/vscode
npm install
```

To launch the Extension Development Host, you need a launch configuration of type `extensionHost` —
pressing F5 with no such config just tries to debug the open file. `.vscode/` is gitignored, so
create `.vscode/launch.json` at the repo root yourself (it opens the repo in the host and points
`--extensionDevelopmentPath` at `editors/vscode`):

```jsonc
{
  "version": "0.2.0",
  "configurations": [
    {
      "name": "Run Pyfun Extension",
      "type": "extensionHost",
      "request": "launch",
      "args": [
        "${workspaceFolder}",
        "--extensionDevelopmentPath=${workspaceFolder}/editors/vscode"
      ]
    }
  ]
}
```

With the repo root open in VS Code, open **Run and Debug** (Ctrl-Shift-D), pick **Run Pyfun
Extension**, and start it (F5).

A second "[Extension Development Host]" window opens with the repo loaded. Open any `.pyfun` file;
diagnostics appear on save/typing and hover shows inferred types. After changing Rust code,
`cargo build` then run **Developer: Reload Window** in the host so it relaunches the server.

## Publishing to the Marketplace

See the VS Code docs on [publishing extensions](https://code.visualstudio.com/api/working-with-extensions/publishing-extension).
In short: create a publisher whose ID matches the `publisher` field in `package.json`, then from this
directory run `npx @vscode/vsce publish`. The `pyfun.server.path` default (`pyfun` on `PATH`) means a
Marketplace install works as soon as the user has run `pip install pyfun-lang`.
