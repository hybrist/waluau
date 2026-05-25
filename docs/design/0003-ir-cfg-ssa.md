# 0003: CFG and SSA IR

## Status

Accepted.

## Goal

Use a conventional control-flow graph IR with a separate SSA construction pass. This provides a straightforward path to verification, testing, and Wasm lowering without the complexity of a graph-only optimizer IR.

## IR Layers

- AST: syntax-oriented, preserves source structure and spans
- typed HIR: name-resolved and type-checked, still structured
- CFG IR: explicit control flow, basic blocks, mutable locals before SSA
- SSA CFG IR: phi nodes and SSA value definitions

The CFG IR is the first backend-facing IR. It should model program semantics clearly and avoid encoding Wasm stack-machine details.

## Function Shape

Each IR function should contain:

- function name or symbol
- typed parameter list
- return type
- entry block ID
- block arena
- local arena

## Basic Blocks

Each basic block should contain:

- block ID
- phi nodes on block entry after SSA construction
- a linear list of instructions
- exactly one terminator
- predecessor list
- successor list

Required terminators:

- `jump`
- `branch`
- `return`
- `unreachable`

## Pre-SSA Instructions

The initial lowering from HIR should use mutable locals. A small instruction set is enough for the first milestone:

- `const.i32`
- `const.u32`
- `const.f32`
- `const.f64`
- `const.bool`
- unary ops
- binary ops
- `local.get`
- `local.set`
- direct call

This is intentionally boring. The point is to make control flow and data flow explicit before converting local state into SSA values.

## SSA Strategy

SSA should be built as a pass over CFG IR, not produced directly by the parser or HIR lowerer.

Use classic phi-node SSA:

1. Lower HIR into CFG with mutable locals.
2. Compute dominators.
3. Insert phi nodes for locals with multiple reaching definitions.
4. Rename local definitions and uses into SSA values.
5. Verify dominance and phi consistency.

This approach is preferred because it is easier to debug and easier to compare against established SSA algorithms and references.

## Verification Requirements

CFG verification must check:

- every block has exactly one terminator
- every referenced block exists
- predecessor and successor lists agree
- every referenced local and instruction exists
- terminators reference valid successor blocks

SSA verification must check:

- every use has a definition
- definitions dominate uses
- phi input count matches predecessor count
- phi predecessor ordering matches block predecessor ordering
- operand types match instruction expectations, including concrete scalar widths
- branch conditions have type `bool`
- return values match the function return type

Without these verifiers, IR bugs become slow, indirect, and difficult to localize.

## Wasm Lowering Constraints

Wasm lowering is a separate step after SSA.

Responsibilities:

- turn CFG structure into structured Wasm control flow
- materialize locals where needed for Wasm emission
- emit scalar arithmetic, comparisons, branches, loops, calls, and returns
- validate the emitted module

The initial Wasm backend should target scalar programs only. `wasm-gc` object features start later with arrays.

## Suggested Rust Shapes

These sketches are not a fixed API, but they capture the intended structure.

```rust
pub struct IrFunction {
    pub entry: BlockId,
    pub blocks: Arena<BlockId, Block>,
    pub locals: Arena<LocalId, LocalDecl>,
    pub params: Vec<Type>,
    pub ret: Type,
}

pub struct Block {
    pub phis: Vec<Phi>,
    pub insts: Vec<InstId>,
    pub term: Terminator,
    pub preds: Vec<BlockId>,
    pub succs: Vec<BlockId>,
}

pub enum Terminator {
    Jump(BlockId),
    Branch { cond: ValueId, then_blk: BlockId, else_blk: BlockId },
    Return(Option<ValueId>),
    Unreachable,
}
```

## Test Cases That Must Exist Early

- branch merge phi
- loop-carried variable phi
- nested conditional renaming
- invalid branch condition type
- early return CFG shape

These tests are part of the architecture, not optional polish. They prove the compiler can survive the first real data-flow and control-flow cases.
