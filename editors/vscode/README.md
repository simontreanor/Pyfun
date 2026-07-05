# Pyfun for VS Code

A minimal VS Code extension that connects to the Pyfun language server
(`pyfun lsp`). All language smarts live in the Rust server (`src/lsp/`); this
client only launches it and wires up document sync.

## Features

- **Diagnostics** — type, effect, unit, and exhaustiveness errors inline, updated
  as you type.
- **Hover** — the inferred type of the expression (or binding) under the cursor,
  with latent effects shown on arrows (e.g. `string ->{io} unit`). Pyfun types
  are never written, so hover is the way to see what the compiler inferred.
- **Go-to-definition** and **find-references** — jump to a binding's definition or
  list all its uses (locals, parameters, pattern vars, and top-level / module
  values).
- **Rename** — rename a local or top-level `let` value across the file.
- **Completion** — module members (`List.map`, `Map.tryFind`), prelude builtins,
  constructors, type names, and keywords.
- **Document symbols** — the file outline, including in-file `module` members.

All of the above run over **resilient analysis**: the lexer and parser both
recover, so a half-typed file still hovers, navigates, and completes.

## Syntax colours

The TextMate grammar (`pyfun.tmLanguage.json`) tags tokens by their semantic
role, and the extension **pins a colour per role** via `configurationDefaults`
so the palette reads the same across themes. The dark values are Monokai's (plus
the bracket-pair gold); light themes get **darkened, same-hue variants** so each
role stays legible on white (WCAG-AA contrast). Control-flow keywords and the
escape hatches stay visibly distinct; pink/magenta is reserved exclusively for
the mutability/FFI escape hatches.

| Role | Tokens | Dark | Light |
| --- | --- | --- | --- |
| Declarations — *introduces a name* | `let` `fun` `type` `measure` `module` | cyan `#66D9EF` | teal `#0E7490` |
| Escape hatches — mutation + FFI | `mut` `<-` `extern` | pink `#F92672` | magenta `#C71D6C` |
| Computation-expression builders | `async` `seq` `result` | purple `#AE81FF` | violet `#7C3AED` |
| Units of measure | `<m>`, `<m/s^2>` | orange `#FD971F` | burnt orange `#C2410C` |
| Control flow + logical | `if` `then` `else` `elif` `match` `case` `with` `return` `yield` `do` `in` · `and` `or` `not` | gold `#FFD700` | amber `#9A6700` |

The light overrides live under a `"[*Light*]"` key, so they apply only to themes
whose name contains *Light*.

Everything else (identifiers, type names, constructors, strings, numbers,
operators) follows your active theme. To override a pin, add your own
`editor.tokenColorCustomizations` rule for the scope (e.g.
`keyword.control.pyfun`) in user settings — it wins over the extension default.

## Install (to just use it)

If you only want the extension working in your editor — not to hack on the
client — build a `.vsix` and install it. This is more reliable than the F5
dev-host flow below.

```bash
cargo build                      # produces target/debug/pyfun(.exe) — the server
cd editors/vscode
npm install
npx @vscode/vsce package         # produces pyfun-<version>.vsix
```

Then install with the CLI of the VS Code variant you actually run — use
`code-insiders` for Insiders, `code` for stable. **Use the `bin/code(.cmd)` CLI
wrapper, not the GUI `code.exe`** (the latter just launches the app instead of
installing). A raw folder copy into the extensions directory does **not** work:
VS Code only loads extensions registered in its `extensions/extensions.json`
manifest, which `--install-extension` updates.

```bash
code-insiders --install-extension pyfun-0.0.1.vsix --force
```

Point the extension at the built server via a setting (see below), then reload
the window. An already-running window won't pick up a newly installed extension
until **Developer: Reload Window**.

## Setup (development)

The extension shells out to a `pyfun` executable. From the repo root:

```bash
cargo build                      # produces target/debug/pyfun
```

Then either put `pyfun` on your `PATH`, or set `pyfun.server.path` in VS Code
settings to the built binary. `${workspaceFolder}` is expanded by the client, so
a checkout-relative path works on any machine:

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

To launch the Extension Development Host, you need a launch configuration of
type `extensionHost` — pressing F5 with no such config just tries to debug the
open file. `.vscode/` is gitignored, so create `.vscode/launch.json` at the repo
root yourself (it opens the repo in the host and points
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

With the repo root open in VS Code, open **Run and Debug** (Ctrl-Shift-D), pick
**Run Pyfun Extension**, and start it (F5).

A second "[Extension Development Host]" window opens with the repo loaded. Open
any `.pyfun` file; diagnostics appear on save/typing and hover shows inferred
types. After changing Rust code, `cargo build` then run **Developer: Reload
Window** in the host so it relaunches the server.

## Scope

The client is deliberately thin — it only launches `pyfun lsp` and wires up
document sync. Every feature above is implemented in the Rust server (`src/lsp/`).
Doc-comment hover (`##` doc comments rendered below the type) landed 2026-07-03.
Truly incremental reparse was decided against — whole-file analysis is
milliseconds at this scale and the server's fingerprint-validated caches already
remove all redundant re-analysis — see `ROADMAP.md`.
