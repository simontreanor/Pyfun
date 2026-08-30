# Releasing Pyfun

The compiler version in `Cargo.toml` is canonical, and **every artifact that carries
the Pyfun version tracks it** — one Pyfun version, the same number everywhere a user
can read one. That set is `Cargo.toml`, `editors/vscode/package.json`,
`editors/jetbrains/build.gradle.kts` and `editors/emacs/pyfun-mode.el`; the wheel and
the Jupyter kernel derive theirs. Two artifacts are deliberately versioned on their
own because they change on their own schedule and a user reads them as a grammar
rather than as a compiler: `editors/zed/extension.toml` and
`editors/tree-sitter-pyfun/tree-sitter.json`, each bumped when the grammar changes.
An editor artifact is never left behind because "it didn't change": a user whose
extension says 0.2.0 while `pyfun --version` says 0.3.0 has no way to tell whether
that is fine or a broken install. Version numbers are cheap; that doubt is not.

## Every release

1. Bump `version` in **all four** files, to the same number:
   - `Cargo.toml` (canonical; run any test to refresh `Cargo.lock`)
   - `editors/vscode/package.json` (+ a `CHANGELOG.md` entry — "no client
     changes" is a fine entry)
   - `editors/jetbrains/build.gradle.kts`
   - `editors/emacs/pyfun-mode.el` (the `;; Version:` header — MELPA reads it)
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
   - **JetBrains**: `./gradlew publishPlugin` in `editors/jetbrains/` (needs
     `JETBRAINS_PERMANENT_TOKEN` and JDK 21; the wrapper fetches Gradle itself,
     so nothing has to be installed) — but see the acceptance gate below. The
     upload then waits on JetBrains moderation, so the plugins API keeps
     reporting the previous version for a while; that is not a failed publish.
     **Use the wrapper, never a bare `gradle`** — Gradle is not installed on the
     dev machine, and each release that reached for it lost time rediscovering
     that.
4. Verify: `pip install "pyfun-lang[jupyter]==X.Y.Z"` in a clean venv, then
   `pip install -U "pyfun-lang[jupyter]"` in the day-to-day Python env so the
   Jupyter kernel is not stranded on the previous compiler.
5. Record it: update the *Distribution* ledger in `ROADMAP.md` with each
   registry's verified version and date (check the APIs, not the previous
   line), in one follow-up PR. It cannot ride the release PR because the
   ledger records what the tag published, which does not exist until after it.
   One ledger PR per release; anything a registry does later (moderation
   clearing, a manual upload) waits for the next substantive change.

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
