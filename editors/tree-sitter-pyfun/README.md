# tree-sitter-pyfun

A [Tree-sitter](https://tree-sitter.github.io/) grammar for
[Pyfun](https://github.com/simontreanor/Pyfun). One artifact serves every
Tree-sitter-based editor: Helix, Zed, Neovim (via nvim-treesitter), and more.

The grammar mirrors the reference implementation in the Pyfun compiler
(`src/lexer/`, `src/parser/`): the full precedence table, all item forms, and
the offside rule — implemented as an external scanner (`src/scanner.c`)
emitting zero-width `indent`/`dedent`/`sep` tokens, with implicit line
continuation inside brackets. `queries/highlights.scm` uses standard capture
names.

The generated parser (`src/parser.c`) is committed, so consumers need only a
C compiler — not the tree-sitter CLI.

## Using it

**Helix** (`~/.config/helix/languages.toml`) — add a grammar source next to
the language entry from [`editors/README.md`](../README.md), then fetch/build:

```toml
[[grammar]]
name = "pyfun"
source = { git = "https://github.com/simontreanor/Pyfun", rev = "main", subpath = "editors/tree-sitter-pyfun" }
```

```bash
hx --grammar fetch && hx --grammar build
mkdir -p ~/.config/helix/runtime/queries/pyfun
cp queries/highlights.scm ~/.config/helix/runtime/queries/pyfun/
```

**Neovim** (nvim-treesitter) — register the parser, then `:TSInstall pyfun`.
The highlight query ships in `editors/nvim/queries/pyfun/`, which you already
have on your runtimepath if you followed `editors/README.md`:

```lua
require('nvim-treesitter.parsers').get_parser_configs().pyfun = {
  install_info = {
    url = 'https://github.com/simontreanor/Pyfun',
    files = { 'src/parser.c', 'src/scanner.c' },
    location = 'editors/tree-sitter-pyfun',
  },
  filetype = 'pyfun',
}
```

## Development

```bash
npm install -g tree-sitter-cli   # or: cargo install tree-sitter-cli
tree-sitter generate             # grammar.js -> src/parser.c
tree-sitter parse test/stress.pyfun
```

The correctness gate: **every `.pyfun` file in the repo must parse with zero
ERROR nodes**, and `test/stress.pyfun` (which covers constructs the examples
don't) must stay accepted by the real compiler:

```powershell
Get-ChildItem ..\..\examples, .\test -Recurse -Filter *.pyfun |
  ForEach-Object { $null = tree-sitter parse $_.FullName --quiet; if ($LASTEXITCODE -ne 0) { $_.FullName } }
cargo run -- check test/stress.pyfun   # from the repo root
```

Known simplifications (all strictly more permissive than the compiler):
triple-quoted f-strings are opaque tokens (no hole highlighting), and the
grammar accepts a superset of layouts in a few extern/type positions where
the reference lexer would reject. The compiler remains the gatekeeper.
