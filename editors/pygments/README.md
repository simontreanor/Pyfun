# pygments-pyfun

A [Pygments](https://pygments.org) lexer for [Pyfun](https://github.com/simontreanor/Pyfun), the
F#-inspired, functional-first language that compiles to readable Python.

Install it next to Pygments and anything that highlights with Pygments recognises Pyfun: Sphinx
and MkDocs code blocks marked `pyfun`, Jupyter's nbconvert exports, and the command line.

```bash
pip install ./editors/pygments        # from a checkout of the Pyfun repository
pygmentize -l pyfun examples/hello.pyfun
```

The package registers the lexer through Pygments' `pygments.lexers` entry point under the alias
`pyfun` and the `*.pyfun` filename pattern, so no configuration is needed.

## What it highlights

The token classes follow the Tree-sitter grammar's highlight query, so a rendered page matches
an editor: keywords (including `let!`/`return!` and `pure`), a computation-expression builder
before its `{`, capitalised constructors and types, qualified members (`List.map`), built-in
types and functions, typed holes (`?name`), type variables (`'a`), effect annotations
(`->{io}`), units of measure after a number (`9.8<m/s^2>`), and every string form: plain,
triple-quoted, raw, and `f"…"` with its `{…}` holes lexed as code.

## Tests

```bash
python tests/test_lexer.py
```

The suite lexes every `.pyfun` file in the repository, checking that no token is an error and
that the tokens reassemble the source exactly, and pins the token class of each construct above.
