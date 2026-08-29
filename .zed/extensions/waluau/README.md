# Waluau for Zed

This repo-local Zed development extension registers `.walu` files as the
**Waluau** language, provides Waluau syntax and structural queries, and connects
them to the compiler's own `waluau-lsp` server. It does not associate Waluau
files with Zed's Lua language.

The syntax parser is pinned to a compatible typed-Luau Tree-sitter grammar.
Waluau owns the editor language definition and adds its contextual keywords,
primitive types, operators, outline, indentation, bracket, and text-object
queries on top of that parsing foundation.

## Install

1. Open Zed's Extensions page (`zed: extensions` in the command palette).
2. Choose **Install Dev Extension**.
3. Select this repository's `.zed/extensions/waluau` directory.
4. Reload the workspace if a `.walu` buffer was already open.

The repository's `.zed/settings.json` selects `Waluau` for `.walu` files and
launches `tools/editors/waluau-lsp`. That launcher builds the language server
on first use and whenever its Rust sources change. Diagnostics, completion,
hover, navigation, and formatting then come from the Waluau compiler itself.

To use a different server executable, override `lsp.waluau-lsp.binary.path` in
your Zed settings.

## Develop

Run the extension checks from the repository root:

```sh
cargo check --manifest-path .zed/extensions/waluau/Cargo.toml
node --test .zed/extensions/waluau/test/*.test.mjs
```

After changing the extension, reinstall it from the Extensions page so Zed
recompiles the WebAssembly module and Tree-sitter grammar.
