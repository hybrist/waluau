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

1. `pnpm install` from the repo root.
2. Open the Extensions view — this extension appears under **Workspace
   Extensions** (VS Code >= 1.89 discovers unpacked extensions in
   `.vscode/extensions/`). Click **Install**, then reload when prompted.

The extension runs the committed `dist/extension.js` bundle, so a checkout is
ready to install without a separate build. Bundling keeps the language client
inside the extension directory, which lets VS Code associate formatting and
other language providers with `waluau-dev.waluau-vscode` even when pnpm stores
the source dependency outside this directory.

Opening any `.walu` file selects Waluau language mode and activates the client.
Diagnostics appear as squiggles and in the Problems panel; server logs are in
the "Waluau Language Server" output channel.

To use a specific server binary instead of the launcher script, set
`waluau.languageServerPath` in your settings.

## Develop

After changing `extension.js` or its dependencies, run `pnpm build` in this
directory and commit the regenerated `dist/extension.js`. `pnpm test` checks
that the committed bundle is current and that `vscode-languageclient` has no
runtime import escaping the extension directory.

Open this folder in VS Code and press F5 ("Run Extension") to launch an
Extension Development Host with the bundled client loaded.
