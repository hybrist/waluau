# Waluau VS Code integration (workspace extension)

Connects VS Code to the repo's own language server (`crates/waluau-lsp`) for
`.walu` files: live multi-error diagnostics with precise spans. Syntax
highlighting comes from VS Code's built-in Lua grammar via the workspace's
`files.associations` setting; this extension only runs the language client.

The server is launched through [`tools/editors/waluau-lsp`](../../../tools/editors/waluau-lsp),
which builds `waluau-lsp` on first use (and after compiler changes), so no
manual build step is needed.

## Install

1. `pnpm install` from the repo root (fetches `vscode-languageclient`).
2. Open the Extensions view — this extension appears under **Workspace
   Extensions** (VS Code >= 1.89 discovers unpacked extensions in
   `.vscode/extensions/`). Click **Install**, then reload when prompted.

Opening any `.walu` file activates the client. Diagnostics appear as
squiggles and in the Problems panel; server logs are in the "Waluau
Language Server" output channel.

To use a specific server binary instead of the launcher script, set
`waluau.languageServerPath` in your settings.

## Develop

Open this folder in VS Code and press F5 ("Run Extension") to launch an
Extension Development Host with the client loaded.
