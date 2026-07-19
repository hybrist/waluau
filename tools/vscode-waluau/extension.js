// Repo-local VS Code client for the Waluau language server. Documents are
// matched by the .walu pattern rather than a language id, so the repo's
// files.associations mapping (*.walu -> lua) keeps VS Code's built-in Lua
// grammar for highlighting while this client provides diagnostics.
const path = require('node:path');
const { workspace, window } = require('vscode');
const { LanguageClient } = require('vscode-languageclient/node');

let client;

async function activate(context) {
  const configured = workspace.getConfiguration('waluau').get('languageServerPath');
  const command = configured && configured.length > 0
    ? configured
    : path.join(context.extensionPath, '..', 'editors', 'waluau-lsp');

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
