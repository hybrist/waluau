# Waluau VS Code integration (repo-local)

Connects VS Code to the repo's own language server (`crates/waluau-lsp`) for
`.walu` files: live multi-error diagnostics with precise spans. Syntax
highlighting comes from VS Code's built-in Lua grammar via the workspace's
`files.associations` setting; this extension only runs the language client.

The server is launched through [`tools/editors/waluau-lsp`](../editors/waluau-lsp),
which builds `waluau-lsp` on first use (and after compiler changes), so no
manual build step is needed.

## Install

```bash
pnpm install                      # from the repo root (workspace dependency)
ln -s "$(pwd)/tools/vscode-waluau" ~/.vscode/extensions/waluau-dev
```

Reload VS Code afterwards. Opening any `.walu` file in this repo activates
the client.

To use a specific server binary instead of the launcher script, set
`waluau.languageServerPath` in your settings.

## Develop

Open this folder in VS Code and press F5 ("Run Extension") to launch an
Extension Development Host with the client loaded.
