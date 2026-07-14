# Editor support

Pyfun ships a **zero-dependency language server** built into the compiler binary: `pyfun lsp`
speaks LSP over stdio. That means editor support is client configuration, not engineering —
any editor with an LSP client gets the full feature set:

- **Diagnostics** as you type (resilient analysis that survives a half-typed file)
- **Hover** showing the inferred type *and* effect of any expression, binding, or parameter
- **Go-to-definition** and **find-references**, across files
- **Rename**, project-wide, for values, constructors, and types
- **Completion**, **document symbols**, and **workspace symbols**

The only prerequisite for every editor below is having `pyfun` on your `PATH`:

```bash
pip install pyfun-lang
pyfun --version
```

(Or build from source: `cargo build --release` in the repo root, then put
`target/release/pyfun` on your `PATH`.)

---

## VS Code

Install **[Pyfun](https://marketplace.visualstudio.com/items?itemName=pyfun.pyfun)** from the
Marketplace (or search "Pyfun" in the Extensions panel). It bundles a syntax-highlighting
grammar and launches `pyfun lsp` automatically. Sources live in [`vscode/`](vscode/);
building from source is covered in [`vscode/DEVELOPMENT.md`](vscode/DEVELOPMENT.md).

---

## Neovim

Two pieces: the runtime files in [`nvim/`](nvim/) (filetype detection + regex syntax
highlighting), and a few lines of Lua to start the server.

**1. Install the runtime files** — copy the `nvim/` folder's contents into your config:

```bash
cp -r editors/nvim/* ~/.config/nvim/
```

(or add this repo's `editors/nvim` directory to your `runtimepath`, e.g. with lazy.nvim:
`{ dir = "/path/to/Pyfun/editors/nvim" }`.)

**2. Start the language server** — Neovim **0.11+**, add to your `init.lua`:

```lua
vim.lsp.config('pyfun', {
  cmd = { 'pyfun', 'lsp' },
  filetypes = { 'pyfun' },
  root_markers = { '.git' },
})
vim.lsp.enable('pyfun')
```

On **Neovim 0.8–0.10**, use an autocmd instead:

```lua
vim.api.nvim_create_autocmd('FileType', {
  pattern = 'pyfun',
  callback = function(ev)
    vim.lsp.start({
      name = 'pyfun',
      cmd = { 'pyfun', 'lsp' },
      root_dir = vim.fs.dirname(vim.fs.find({ '.git' }, { upward = true })[1] or ev.file),
    })
  end,
})
```

That's it — open a `.pyfun` file and hover (`K`), go-to-definition (`gd` / `C-]`),
rename (`grn`), and diagnostics all work.

**Optional: Tree-sitter highlighting** — the bundled regex syntax is serviceable, but
the [Tree-sitter grammar](tree-sitter-pyfun/) is precise (it understands the offside
rule). With nvim-treesitter installed, register the parser and `:TSInstall pyfun`;
the highlight query is already in `editors/nvim/queries/pyfun/`:

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

---

## Helix

Add to `~/.config/helix/languages.toml` (on Windows: `%AppData%\helix\languages.toml` —
Helix does not honor `XDG_CONFIG_HOME` there):

```toml
[language-server.pyfun]
command = "pyfun"
args = ["lsp"]

[[language]]
name = "pyfun"
scope = "source.pyfun"
file-types = ["pyfun"]
comment-token = "#"
indent = { tab-width = 4, unit = "    " }
language-servers = ["pyfun"]
```

All LSP features work (diagnostics, hover, goto, rename, completion). For syntax
highlighting, add the [Tree-sitter grammar](tree-sitter-pyfun/) and build it:

```toml
[[grammar]]
name = "pyfun"
source = { git = "https://github.com/simontreanor/Pyfun", rev = "main", subpath = "editors/tree-sitter-pyfun" }
```

```bash
hx --grammar fetch && hx --grammar build
mkdir -p ~/.config/helix/runtime/queries/pyfun
cp editors/tree-sitter-pyfun/queries/highlights.scm ~/.config/helix/runtime/queries/pyfun/
```

---

## Emacs

Emacs 29+ has eglot built in. Add to your `init.el`:

```elisp
(define-derived-mode pyfun-mode prog-mode "Pyfun"
  "Major mode for Pyfun source files."
  (setq-local comment-start "# ")
  (setq-local comment-start-skip "#+\\s-*"))

(add-to-list 'auto-mode-alist '("\\.pyfun\\'" . pyfun-mode))

(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs '(pyfun-mode . ("pyfun" "lsp"))))

(add-hook 'pyfun-mode-hook #'eglot-ensure)
```

For `lsp-mode` instead of eglot:

```elisp
(with-eval-after-load 'lsp-mode
  (add-to-list 'lsp-language-id-configuration '(pyfun-mode . "pyfun"))
  (lsp-register-client
   (make-lsp-client :new-connection (lsp-stdio-connection '("pyfun" "lsp"))
                    :activation-fn (lsp-activate-on "pyfun")
                    :server-id 'pyfun)))
(add-hook 'pyfun-mode-hook #'lsp)
```

---

## Zed

A full extension lives in [`zed/`](zed/) — Tree-sitter highlighting, outline,
and the language server. Until it reaches the Zed registry, install it as a dev
extension: **Extensions → Install Dev Extension** → select the `editors/zed`
directory of a clone (Zed compiles it locally; needs a Rust toolchain). See
[`zed/README.md`](zed/README.md).

---

## PyCharm / IntelliJ (including free mode and Community editions)

Two pieces, both zero-code: an LSP client and syntax highlighting.

**1. Language server via [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij)**
(Red Hat's free, open-source LSP client — works in unified PyCharm's free mode *and* the
legacy Community editions):

1. Install **LSP4IJ** from the plugin marketplace.
2. **Settings → Languages & Frameworks → Language Servers**, click **+** to open the
   *New Language Server* dialog.
3. *Server* tab — **Name:** `pyfun`, **Command:** `pyfun lsp`
4. *Mappings* tab — add a **File name pattern**: `*.pyfun` with **Language Id** `pyfun`.

Open a `.pyfun` file: diagnostics, hover types/effects, go-to-definition, rename, and
completion all work.

**2. Syntax highlighting via the built-in TextMate Bundles support** (the IDE loads
VS Code-format extensions directly):

1. Clone this repo (or copy the [`vscode/`](vscode/) folder).
2. **Settings → Editor → TextMate Bundles**, click **+**, and select the `editors/vscode`
   directory.

> On unified PyCharm/IntelliJ 2025.2+, JetBrains' native LSP API is also free for all
> users, so a dedicated marketplace plugin is possible — it's on the roadmap for when
> there's demand; the LSP4IJ route above needs nothing from us.

---

## Any other editor

If your editor has an LSP client, point it at:

| Setting   | Value           |
|-----------|-----------------|
| Command   | `pyfun`         |
| Arguments | `lsp`           |
| Transport | stdio           |
| Filetypes | `*.pyfun`       |

The server is stateless to configure — no settings, no init options, no separate install.
