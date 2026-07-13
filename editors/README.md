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

---

## Helix

Add to `~/.config/helix/languages.toml`:

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

All LSP features work (diagnostics, hover, goto, rename, completion). Helix highlights
via Tree-sitter, and Pyfun doesn't ship a Tree-sitter grammar yet — so you get a plain
color scheme until one lands (it's on the roadmap).

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

## Any other editor

If your editor has an LSP client, point it at:

| Setting   | Value           |
|-----------|-----------------|
| Command   | `pyfun`         |
| Arguments | `lsp`           |
| Transport | stdio           |
| Filetypes | `*.pyfun`       |

The server is stateless to configure — no settings, no init options, no separate install.
