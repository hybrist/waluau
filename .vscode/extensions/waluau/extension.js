// Repo-local VS Code client for the Waluau language server, installed as a
// workspace extension from .vscode/extensions/waluau. Documents are matched
// by the .walu pattern rather than a language id, so the repo's
// files.associations mapping (*.walu -> lua) keeps VS Code's built-in Lua
// grammar for highlighting while this client provides diagnostics.
const fs = require('node:fs');
const path = require('node:path');
const { workspace, window } = require('vscode');
const { LanguageClient } = require('vscode-languageclient/node');

let client;

function launcherPath(context) {
  const configured = workspace.getConfiguration('waluau').get('languageServerPath');
  if (configured && configured.length > 0) return configured;
  // Prefer the launcher inside the open workspace (the repo checkout); the
  // extension itself may run from a copied location after install.
  for (const folder of workspace.workspaceFolders ?? []) {
    const candidate = path.join(folder.uri.fsPath, 'tools', 'editors', 'waluau-lsp');
    if (fs.existsSync(candidate)) return candidate;
  }
  return path.join(context.extensionPath, '..', '..', '..', 'tools', 'editors', 'waluau-lsp');
}

async function activate(context) {
  const command = launcherPath(context);

  client = new LanguageClient(
    'waluau-lsp',
    'Waluau Language Server',
    { command, args: [] },
    {
      documentSelector: [{ pattern: '**/*.walu' }],
      outputChannel: window.createOutputChannel('Waluau Language Server'),
    },
  );
  await client.start();
  context.subscriptions.push({ dispose: () => client?.stop() });
}

function deactivate() {
  return client?.stop();
}

module.exports = { activate, deactivate };
