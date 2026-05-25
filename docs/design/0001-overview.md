# 0001: Waluau Overview

## Status

Accepted.

## Goal

Build a fast ahead-of-time compiler for a Lua-like language targeting Wasm, with a conventional CFG+SSA middle-end and a phased rollout into `wasm-gc` features.

## Repository Shape

The repository should use a monorepo layout from the beginning so the project can add more Rust crates and non-Rust pieces without a later reshuffle.

```text
/
  Cargo.toml
  Cargo.lock
  rust-toolchain.toml
  crates/
    waluau-cli/
    waluau-driver/
    waluau-span/
    waluau-diagnostics/
    waluau-ast/
    waluau-lexer/
    waluau-parser/
    waluau-hir/
    waluau-ir/
    waluau-codegen-wasm/
  docs/
    design/
      0001-overview.md
      0002-language-v0.md
      0003-ir-cfg-ssa.md
  tests/
    fixtures/
    integration/
  runtime/
    README.md
```

The initial workspace should be conservative, not over-abstracted. Crates exist to stabilize the top-level shape and keep dependencies one-way, not to turn a greenfield compiler into a framework.

## Crate Roles

- `waluau-cli`: command-line entrypoint
- `waluau-driver`: full-pipeline orchestration
- `waluau-span`: spans, file IDs, and shared source location types
- `waluau-diagnostics`: compiler diagnostics and rendering
- `waluau-ast`: syntax tree definitions
- `waluau-lexer`: tokenization
- `waluau-parser`: AST construction from tokens
- `waluau-hir`: name resolution, type checking, and typed HIR
- `waluau-ir`: CFG IR, SSA construction, verification, and dumps
- `waluau-codegen-wasm`: Wasm lowering, emission, and validation

## Initial Pipeline

The compiler pipeline is:

```text
source
-> lexer
-> parser
-> AST
-> name resolution
-> typed HIR
-> CFG IR
-> SSA construction
-> SSA verification
-> Wasm lowering
-> Wasm validation
```

This intentionally avoids extra IR layers and avoids an early optimization framework. The first milestone is correctness and debuggability, not maximal flexibility.

## Design Constraints

- Use a Lua-like surface syntax, not full Lua semantics.
- Require explicit type annotations in the initial language slice.
- Keep conditions strictly `bool` in v0.
- Start with scalar Wasm before `wasm-gc` object features.
- Add verification early for both CFG and SSA invariants.
- Keep Wasm stack-machine details out of the CFG IR.

## Feature Ladder

- M0: numbers, booleans, locals, functions, calls, returns
- M1: control flow, mutable locals, CFG lowering, SSA construction
- M2: arrays and the first `wasm-gc` heap-backed representation work
- M3: bytes and strings
- M4: tables
- M5: metatables
- M6: `require` and modules

This order is deliberate. Arrays are the first point where heap representation and `wasm-gc` matter. Tables and metatables stay late because they infect semantics and optimizer assumptions.

## Immediate Backlog

- Scaffold the Cargo workspace under `crates/`
- Implement the v0 frontend
- Implement CFG and SSA IR with verification
- Add scalar Wasm code generation and end-to-end tests

These work items are tracked in beads under the bootstrap epic.
