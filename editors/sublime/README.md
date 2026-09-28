# Pyfun for Sublime Text

The `Pyfun/` folder is a Sublime Text package: syntax highlighting, `#` comment toggling,
offside-rule indentation, and four-space indentation for `.pyfun` files. The language server
(diagnostics, hover, go-to-definition, rename, completion) runs through the
[LSP](https://packagecontrol.io/packages/LSP) package.

## Install

1. In Sublime Text, choose **Preferences > Browse Packages…** and copy the `Pyfun` folder there
   (or symlink it, so it follows this repository).
2. For the language server, install `pyfun` (`pip install pyfun-lang`) and the **LSP** package
   from Package Control, then copy the `clients` entry from
   [`Pyfun/LSP-pyfun.sublime-settings`](Pyfun/LSP-pyfun.sublime-settings) into
   **Preferences > Package Settings > LSP > Settings**.

Open a `.pyfun` file and the syntax is picked automatically.

## What is in the package

| File | Purpose |
| --- | --- |
| `Pyfun.tmLanguage` | the syntax, generated from the VS Code extension's TextMate grammar |
| `Comments.tmPreferences` | `#` for **Toggle Comment** |
| `Indentation Rules.tmPreferences` | indent after `=`, `->`, `:`, `then`, `else` and an open bracket |
| `Pyfun.sublime-settings` | four-space indentation |
| `LSP-pyfun.sublime-settings` | the LSP client entry for `pyfun lsp` |

The syntax is not edited by hand. `editors/vscode/pyfun.tmLanguage.json` is the one TextMate
grammar Pyfun maintains, and `python editors/sublime/build.py` writes it out in the property-list
form Sublime reads. CI runs `build.py --check`, so the two cannot drift apart.
