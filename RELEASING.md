# Releasing Pyfun

The compiler version in `Cargo.toml` is canonical, and **every versioned artifact
tracks it** — one Pyfun version, the same number everywhere a user can read one.
An editor artifact is never left behind because "it didn't change": a user whose
extension says 0.2.0 while `pyfun --version` says 0.3.0 has no way to tell whether
that is fine or a broken install. Version numbers are cheap; that doubt is not.

## Every release

1. Bump `version` in **all four** files, to the same number:
   - `Cargo.toml` (canonical; run any test to refresh `Cargo.lock`)
   - `editors/vscode/package.json` (+ a `CHANGELOG.md` entry — "no client
     changes" is a fine entry)
   - `editors/jetbrains/build.gradle.kts`
2. Commit, `git tag vX.Y.Z`, push the tag → `wheels.yml` publishes `pyfun-lang`
   to PyPI (Trusted Publishing) and attaches the `.vsix` to the GitHub release.
   **The attached `.vsix` is named from `package.json`, not the tag** — bumping in
   step 1 is what keeps the release page coherent.
3. Publish the editor artifacts (they do not ride the tag):
   - `npx @vscode/vsce package` in `editors/vscode/`, then **VS Code
     Marketplace**: upload the `.vsix` via the web UI
     (https://marketplace.visualstudio.com/manage/publishers/pyfun — the CLI auth
     path is broken; do not fight it).
   - **Open VSX**: `npx ovsx publish <vsix> -p $OPEN_VSX_APIKEY`.
   - **JetBrains**: `gradle publishPlugin` (needs `JETBRAINS_PERMANENT_TOKEN`;
     JDK 21) — but see the acceptance gate below.
4. Verify: `pip install "pyfun-lang[jupyter]==X.Y.Z"` in a clean venv, then
   `pip install -U "pyfun-lang[jupyter]"` in the day-to-day Python env so the
   Jupyter kernel is not stranded on the previous compiler.

**Follows automatically — no action:** Mason registry (Renovate bumps the pinned
PyPI version), MELPA (builds `pyfun-mode.el` from `main` HEAD), nvim-lspconfig
(unversioned), the Jupyter kernel (reads the installed wheel's version).

## Only when the Tree-sitter grammar changed (`editors/tree-sitter-pyfun/`)

The grammar is pinned by commit in several places; bump the pins:

1. `editors/zed/extension.toml` — update `rev`, bump the extension `version`,
   then PR the new submodule pin to `zed-industries/extensions`.
2. Helix — PR the new `rev` in upstream `languages.toml`.
3. nvim-treesitter — PR the new revision in their parser registry.
4. Keep `editors/nvim/queries/pyfun/highlights.scm` and
   `editors/zed/languages/pyfun/highlights.scm` in sync with
   `editors/tree-sitter-pyfun/queries/highlights.scm`.

> **Gate on acceptance.** Steps 1–3 above, and the JetBrains publish in "Every
> release", apply **only to registries whose initial submission has been
> accepted** — check the ROADMAP *Distribution* ledger for current status before
> opening any follow-up PR or pushing an update. Never send version/rev bumps to
> a registry whose first submission is still in review (it reads as pestering) or
> was declined (nvim-lspconfig and Mason are deferred until Pyfun has adoption
> evidence; nvim-treesitter upstream is archived with no successor). Bumping the
> version *in the repo* is never gated — only publishing is, so the in-repo
> number stays true to the release even when a registry is not ready for it.
> One-time PR blockers already handled: Zed's CLA is signed; MELPA wanted the
> `Assisted-by:` header that's now in `pyfun-mode.el`.

Tokens live in the gitignored `editors/.env` (`OPEN_VSX_APIKEY`,
`JETBRAINS_PERMANENT_TOKEN`). A tag is irreversible — versions on PyPI cannot
be reused after a yank.
