# 0006: Multi-Value Returns and Assignment (M3)

## Status

Proposed.

## Goal

Define end-to-end semantics and backend representation for:

- multi-value function returns
- multi-value `return` statements
- multi-binding local declarations
- multi-assignment
- multi-value argument expansion at call sites

This note closes the gap between parser/HIR support and IR/Wasm lowering.

## Source Semantics

## Function Signatures

Functions may return a scalar type or an ordered multi-value type list:

- scalar: `function f(): i32`
- multi: `function pair(): i32, bool`

The return list order is semantic.

## Return Statements

- `return expr` is valid for scalar-returning functions.
- `return e1, e2, ...` is valid for multi-returning functions.
- Arity must match exactly.
- Each returned value must be assignable to the corresponding slot type using existing non-lossy numeric widening rules.

## Multi-Binding Locals and Assignment

- `local a: T1, b: T2 = rhs...`
- `a, b = rhs...`

Rules:

- RHS is evaluated left-to-right.
- RHS values are flattened using call expansion:
  - scalar expression contributes one value
  - multi-value expression contributes all its values in order
- Flattened RHS arity must match LHS arity exactly.
- Slot-wise type assignability applies per position.
- Const rebinding rules apply per target in multi-assignment.

## Call Argument Expansion

At call sites, argument expressions are flattened with the same rules:

- `sum2(pair(x, y))` is legal if `pair` returns two values and `sum2` expects two params.

Call arity is checked after flattening.

## IR Model Changes

Current IR models one result per instruction/value and a single `Return(ValueId)`. That is insufficient for multi-value flow.

M3 requires:

- terminator:
  - `Return(Vec<ValueId>)`
- function signature:
  - `return_types: Vec<Type>` (single-return remains `len == 1`)
- call instruction forms:
  - direct call produces `Vec<ValueId>` results
  - indirect call produces `Vec<ValueId>` results
- verifier updates:
  - return arity/type per slot
  - call result count/type per signature
  - dominance checks for each returned result value

To keep SSA simple, represent multi-result-producing instructions explicitly instead of encoding a synthetic tuple value.

## Wasm Lowering

Wasm supports multi-result function types natively.

Lowering rules:

- function type section emits `results` as N wasm types (not just one)
- return terminator emits each return value in order, then `return`
- direct/indirect call emission:
  - call pushes N results
  - each result is immediately stored into an assigned local for its SSA value id
- local planning must allocate slots per IR result value id

No tuple boxing/unboxing is introduced for M3.

## Diagnostics

Required stable diagnostics:

- return arity mismatch
- multi-assignment arity mismatch
- multi-binding declaration arity mismatch
- slot type mismatch with 1-based position
- call arity mismatch after flattening

## Test Matrix

- parser/HIR success:
  - multi return + multi assignment roundtrip
  - call argument expansion from multi-return call
- HIR failures:
  - return arity mismatch
  - assignment arity mismatch
  - slot type mismatch
- IR verifier:
  - return slot type mismatch
  - return arity mismatch
- codegen/driver e2e:
  - function returning two scalars consumed by caller
  - nested multi-return forwarding
  - multi-assignment with swapped values

## Non-Goals

- varargs
- Lua-compatible tail-fill/drop behavior
- runtime tuple objects for multi-values

