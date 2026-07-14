//! Zed extension for Pyfun: locates the `pyfun` binary and launches the
//! bundled language server (`pyfun lsp`, stdio). Grammar and queries are
//! declared in extension.toml / languages/pyfun/.

use zed_extension_api::{self as zed, Result};

struct PyfunExtension;

impl zed::Extension for PyfunExtension {
    fn new() -> Self {
        PyfunExtension
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let command = worktree.which("pyfun").ok_or_else(|| {
            "cannot find the `pyfun` binary on PATH (install with `pip install pyfun-lang`)"
                .to_string()
        })?;
        Ok(zed::Command {
            command,
            args: vec!["lsp".to_string()],
            env: Vec::new(),
        })
    }
}

zed::register_extension!(PyfunExtension);
