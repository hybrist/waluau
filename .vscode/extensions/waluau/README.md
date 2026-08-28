# Waluau VS Code integration (workspace extension)

Connects VS Code to the repo's own language server (`crates/waluau-lsp`) for
`.walu` files. The extension registers the **Waluau** language, adds syntax
highlighting for Waluau declarations, types, and operators on top of VS Code's
Lua grammar, and runs the language client for diagnostics, completion, hover,
navigation, and formatting.

The server is launched through [`tools/editors/waluau-lsp`](../../../tools/editors/waluau-lsp),
which builds `waluau-lsp` on first use (and after compiler changes), so no
manual build step is needed.

## Install

1. `pnpm install` from the repo root (fetches `vscode-languageclient`).
2. Open the Extensions view — this extension appears under **Workspace
   Extensions** (VS Code >= 1.89 discovers unpacked extensions in
   `.vscode/extensions/`). Click **Install**, then reload when prompted.

Opening any `.walu` file selects Waluau language mode and activates the client.
Diagnostics appear as squiggles and in the Problems panel; server logs are in
the "Waluau Language Server" output channel.

To use a specific server binary instead of the launcher script, set
`waluau.languageServerPath` in your settings.

## Develop

Open this folder in VS Code and press F5 ("Run Extension") to launch an
Extension Development Host with the client loaded.
