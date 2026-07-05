# Pyfun for VS Code

Language support for [Pyfun](https://github.com/simontreanor/Pyfun), an F#-inspired,
functional-first language that compiles to readable Python. The extension is a thin client
for the Pyfun language server, so every feature below is computed by the compiler itself.

## Requirements

This extension needs the Pyfun compiler, which provides the language server (`pyfun lsp`).
Install it from PyPI (Python 3.12+):

```
pip install pyfun-lang
```

Once `pyfun` is on your `PATH`, the extension launches the server automatically the first time
you open a `.pyfun` file. If your `pyfun` lives somewhere else, point the extension at it with the
`pyfun.server.path` setting.

## Features

- **Diagnostics** — type, effect, unit, and exhaustiveness errors inline, updated as you type.
- **Hover** — the inferred type of the expression or binding under the cursor, with latent effects
  shown on arrows (e.g. `string ->{io} unit`). Pyfun types are never written, so hover is how you
  see what the compiler inferred. Doc comments (`##`) render below the type.
- **Go-to-definition** and **find-references** — across files, for locals, parameters, pattern
  variables, top-level and module values, constructors, and type names.
- **Rename** — project-wide, for values, constructors, and types.
- **Completion** — module members (`List.map`, `Map.tryFind`), prelude builtins, constructors,
  type names, and keywords.
- **Document and workspace symbols** — the file outline (including in-file `module` members) and a
  project-wide symbol search.

All of the above run over **resilient analysis**: the lexer and parser both recover, so a half-typed
file still hovers, navigates, and completes.

## Syntax colours

The TextMate grammar tags tokens by their semantic role, and the extension **pins a colour per role**
so the palette reads the same across themes. The dark values are Monokai's (plus bracket-pair gold);
light themes get darkened, same-hue variants so each role stays legible on white (WCAG-AA contrast).
Pink/magenta is reserved exclusively for the mutability and FFI escape hatches.

| Role | Tokens | Dark | Light |
| --- | --- | --- | --- |
| Declarations (introduces a name) | `let` `fun` `type` `measure` `module` | cyan `#66D9EF` | teal `#0E7490` |
| Escape hatches (mutation + FFI) | `mut` `<-` `extern` | pink `#F92672` | magenta `#C71D6C` |
| Computation-expression builders | `async` `seq` `result` | purple `#AE81FF` | violet `#7C3AED` |
| Units of measure | `<m>`, `<m/s^2>` | orange `#FD971F` | burnt orange `#C2410C` |
| Control flow + logical | `if` `then` `else` `elif` `match` `case` `with` `return` `yield` `do` `in` · `and` `or` `not` | gold `#FFD700` | amber `#9A6700` |

Everything else (identifiers, type names, constructors, strings, numbers, operators) follows your
active theme. To override a pin, add your own `editor.tokenColorCustomizations` rule for the scope
(e.g. `keyword.control.pyfun`) in user settings; it wins over the extension default.

## Settings

| Setting | Default | Description |
| --- | --- | --- |
| `pyfun.server.path` | `pyfun` | Path to the `pyfun` executable used to launch the language server. Supports `${workspaceFolder}`. |

## Learn more

- [Pyfun on GitHub](https://github.com/simontreanor/Pyfun) — README, examples, and the design document.
- [`pyfun-lang` on PyPI](https://pypi.org/project/pyfun-lang/) — the compiler this extension drives.

Building the extension from source, or hacking on the client, is covered in
[`DEVELOPMENT.md`](https://github.com/simontreanor/Pyfun/blob/main/editors/vscode/DEVELOPMENT.md).

## License

[Apache-2.0](https://github.com/simontreanor/Pyfun/blob/main/LICENSE).
