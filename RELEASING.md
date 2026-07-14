# Releasing Pyfun

The compiler version in `Cargo.toml` is canonical. Everything else either follows
it automatically or is an independently-versioned editor artifact — the tiers
below say which is which, so nothing is missed and nothing is bumped needlessly.

## Every release (compiler / stdlib / kernel changes)

1. Bump `version` in `Cargo.toml`; run any test to refresh `Cargo.lock`.
2. Commit, `git tag vX.Y.Z`, push the tag → `wheels.yml` publishes `pyfun-lang`
   to PyPI (Trusted Publishing) and attaches the `.vsix` to the GitHub release.
3. Verify: `pip install "pyfun-lang[jupyter]==X.Y.Z"` in a clean venv.

**Follows automatically — no action:** Mason registry (Renovate bumps the pinned
PyPI version), MELPA (builds `pyfun-mode.el` from `main` HEAD), nvim-lspconfig
(unversioned), the Jupyter kernel (reads the installed wheel's version).

## Only when the VS Code extension changed (`editors/vscode/`)

1. Bump `version` in `editors/vscode/package.json`.
2. `npx @vscode/vsce package`, then:
   - **VS Code Marketplace**: upload the `.vsix` via the web UI
     (https://marketplace.visualstudio.com/manage/publishers/pyfun — the CLI
     auth path is broken; do not fight it).
   - **Open VSX**: `npx ovsx publish <vsix> -p $OPEN_VSX_APIKEY`.

## Only when the JetBrains plugin changed (`editors/jetbrains/`)

1. Bump `version` in `editors/jetbrains/build.gradle.kts`.
2. `gradle publishPlugin` (needs `JETBRAINS_PERMANENT_TOKEN`; JDK 21).

## Only when the Tree-sitter grammar changed (`editors/tree-sitter-pyfun/`)

The grammar is pinned by commit in several places; bump the pins:

1. `editors/zed/extension.toml` — update `rev`, bump the extension `version`,
   then PR the new submodule pin to `zed-industries/extensions`.
2. Helix — PR the new `rev` in upstream `languages.toml` (once Pyfun ships in
   Helix).
3. nvim-treesitter — PR the new revision in their parser registry (once merged
   there).
4. Keep `editors/nvim/queries/pyfun/highlights.scm` and
   `editors/zed/languages/pyfun/highlights.scm` in sync with
   `editors/tree-sitter-pyfun/queries/highlights.scm`.

Tokens live in the gitignored `editors/.env` (`OPEN_VSX_APIKEY`,
`JETBRAINS_PERMANENT_TOKEN`). A tag is irreversible — versions on PyPI cannot
be reused after a yank.
