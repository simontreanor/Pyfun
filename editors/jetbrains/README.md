# Pyfun for JetBrains IDEs

A thin marketplace plugin: registers the `.pyfun` file type, ships the TextMate
grammar (sourced from [`editors/vscode/`](../vscode/) at build time — single
source of truth), and wires the `pyfun lsp` language server through
[LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij), so it works in
**unified PyCharm free mode and the legacy Community editions** (2024.2+).

Users need `pyfun` on PATH (`pip install pyfun-lang`) or `PYFUN_BIN` set;
installing this plugin auto-installs LSP4IJ as a dependency.

## Build

Requires JDK 21. Gradle itself does not need installing: the committed wrapper
downloads the pinned version on first use. From this directory:

```bash
./gradlew buildPlugin        # -> build/distributions/pyfun-jetbrains-<version>.zip
./gradlew runIde             # launch a sandbox IDE with the plugin for testing
./gradlew verifyPluginStructure
```

## Publish

- **First upload must be manual** (JetBrains rule — it sets the license and
  repository options): log in at
  [plugins.jetbrains.com](https://plugins.jetbrains.com) → **Upload plugin** →
  select the zip, license **Apache-2.0**, category **Programming Language**.
  Moderation takes ~1–3 business days.
- **Every later release**: bump `version` in `build.gradle.kts`, then
  `JETBRAINS_PERMANENT_TOKEN=<token> ./gradlew publishPlugin`.
