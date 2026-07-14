# Pyfun for Zed

Zed extension for [Pyfun](https://github.com/simontreanor/Pyfun): Tree-sitter
syntax highlighting (sourced from
[`editors/tree-sitter-pyfun`](../tree-sitter-pyfun/)), outline, brackets, and
the `pyfun lsp` language server (diagnostics, hover types/effects,
go-to-definition, rename, completion).

Prerequisite: `pyfun` on your `PATH` (`pip install pyfun-lang`).

## Install (dev extension)

Until the extension is on the Zed registry, install it as a dev extension —
Zed compiles it locally (needs a Rust toolchain **with the `wasm32-wasip2`
target**: `rustup target add wasm32-wasip2`, or the install fails with
"failed to compile Rust extension"):

1. Clone this repo.
2. In Zed: **Extensions** (`ctrl-shift-x` / `cmd-shift-x`) → **Install Dev
   Extension** → select the `editors/zed` directory.
3. Open a `.pyfun` file.

## Maintenance notes

- `extension.toml` pins the grammar by commit (`rev`) with `path` pointing at
  the grammar's subdirectory of this repo — bump `rev` when the grammar
  changes.
- `languages/pyfun/highlights.scm` is the Zed-dialect twin of
  `editors/tree-sitter-pyfun/queries/highlights.scm`; keep them in sync.
- Registry publication = PR to
  [zed-industries/extensions](https://github.com/zed-industries/extensions)
  adding this directory as a submodule entry.
