// Minimal VS Code client for the Pyfun language server. It launches `pyfun lsp`
// (a stdio server) and lets the standard LanguageClient drive document sync,
// diagnostics, and hover — so the whole feature surface lives in the Rust server,
// not here. See `DESIGN.md` §9.

const { workspace } = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function activate(context) {
  // The server command is configurable so a checkout can point at
  // target/debug/pyfun without installing anything on PATH.
  const configured = workspace
    .getConfiguration("pyfun")
    .get("server.path", "pyfun");
  // VS Code only expands ${workspaceFolder} in launch.json / tasks.json, not in
  // arbitrary setting values — so expand it ourselves, letting a committed path
  // like "${workspaceFolder}/target/debug/pyfun.exe" stay machine-independent.
  const root = workspace.workspaceFolders?.[0]?.uri.fsPath ?? "";
  const command = configured.replace(/\$\{workspace(?:Folder|Root)\}/g, root);

  const serverOptions = {
    run: { command, args: ["lsp"], transport: TransportKind.stdio },
    debug: { command, args: ["lsp"], transport: TransportKind.stdio },
  };

  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "pyfun" }],
  };

  client = new LanguageClient(
    "pyfun",
    "Pyfun Language Server",
    serverOptions,
    clientOptions,
  );

  client.start();
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
