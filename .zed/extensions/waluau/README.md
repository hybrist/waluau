# Waluau for Zed

This repo-local Zed development extension registers `.walu` files as the
**Waluau** language, provides Waluau syntax and structural queries, and connects
them to the compiler's own `waluau-lsp` server. It does not associate Waluau
files with Zed's Lua language.

Syntax comes from the Waluau tree-sitter grammar in this repository
(`tools/tree-sitter-waluau`), written against the compiler's own lexer and
parser. Every Waluau construct — `const` declarations, `type`/`enum`
declarations, interface conformance, tagged unions, `::` casts, `is` tests,
`match` statements, if-cast bindings, `declare` directives, backtick
interpolation, bytes literals — is a first-class node in the tree, and the
highlight, outline, indentation, bracket, fold, and text-object queries are
written directly against those nodes.

## Install

1. Open Zed's Extensions page (`zed: extensions` in the command palette).
2. Choose **Install Dev Extension**.
3. Select this repository's `.zed/extensions/waluau` directory.
4. Reload the workspace if a `.walu` buffer was already open.

Zed fetches the grammar from this repository at the commit pinned in
`extension.toml` (`[grammars.waluau]`), so installing needs the pinned commit
to exist on GitHub. After changing the grammar, land the grammar change first,
then bump the `commit` pin and reinstall the extension.

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

Grammar development lives in `tools/tree-sitter-waluau`; see its README for
regenerating the parser and running the grammar's own test corpus.

After changing the extension, reinstall it from the Extensions page so Zed
recompiles the WebAssembly module and Tree-sitter grammar.
