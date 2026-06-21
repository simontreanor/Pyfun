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

## Setup (development)

The extension shells out to a `pyfun` executable. From the repo root:

```bash
cargo build                      # produces target/debug/pyfun
```

Then either put `pyfun` on your `PATH`, or set `pyfun.server.path` in VS Code
settings to the built binary, e.g.:

```jsonc
{
  "pyfun.server.path": "C:/git/Pyfun/target/debug/pyfun"
}
```

Install the client dependency and launch the Extension Development Host:

```bash
cd editors/vscode
npm install
code .            # then press F5 to run the extension
```

Open any `.pyfun` file; diagnostics appear on save/typing and hover shows
inferred types.

## Scope

This is the first LSP slice (diagnostics + hover). Go-to-definition,
completion, and effect/hover refinements are future work — see `ROADMAP.md` #10.
