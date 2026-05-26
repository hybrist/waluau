# 0004: Arrays for M2 and Initial Wasm-GC Representation

## Status

Accepted.

## Goal

Define the first heap-backed language feature for M2: fixed-element arrays with explicit element types and a concrete initial `wasm-gc` lowering model.

This note locks syntax, typing, diagnostics, and runtime representation choices before parser/HIR/IR/codegen implementation.

## Scope

Included in M2:

- array type syntax
- array literals
- array element read
- array element write
- `len(...)` builtin on arrays
- bounds checks in generated code
- initial `wasm-gc` array-based representation

Explicitly out of scope for M2:

- table/object syntax
- heterogeneous arrays
- slicing
- iterator protocol
- append/grow/shrink operations
- multidimensional array literals or indexing sugar
- metatable behavior
- module/runtime library surface beyond `len`

## Source Syntax

Array type:

```lua
[T]
```

Examples:

- `[i32]`
- `[bool]`
- `[[i32]]`

Array literal:

```lua
[expr1, expr2, expr3]
```

Indexing:

```lua
arr[idx]
arr[idx] = value
```

Length builtin:

```lua
len(arr)
```

The parser treats `len` as a reserved builtin name in M2 to avoid dynamic global resolution for this operation.

## Type System Rules

## Array Element Type

- `[T]` is a first-class type where `T` is any currently legal non-`nil` value type.
- Element type equality is invariant: `[i32]` is not assignable to `[i64]`, and vice versa.

## Literal Typing

- Empty array literals are rejected in M2 because they do not provide enough information to infer `T`.
- Non-empty array literals require a single element type after existing numeric widening rules are applied.
- If no non-lossy common type exists, emit a type error on the literal.

Examples:

- `[1, 2, 3]` -> `[i32]`
- `[1, 2_i64]` (or equivalent typed source forms) -> `[i64]`
- `[1, true]` -> error
- `[]` -> error (requires future typed-empty syntax)

## Indexing Rules

- Index expression must type-check as `i32`.
- `arr[idx]` has type `T` when `arr: [T]`.
- `arr[idx] = v` requires `v` assignable to `T` under the same non-lossy assignment rules as locals/returns.

## Builtin `len`

- `len([T]) -> i32`.
- Passing non-array values to `len` is a type error.

## Diagnostics Expectations

Required diagnostic classes:

- invalid array element type usage (if type form is unsupported)
- heterogeneous array literal with no valid common element type
- empty array literal without explicit type context
- non-`i32` index expression
- assignment type mismatch on element write
- non-array operand for indexing or `len`

Diagnostics should point at the specific failing element/index/value expression, not only the enclosing statement.

## Runtime Semantics

## Allocation Model

- Array literals allocate a fresh array value.
- Arrays have reference semantics: assigning an array variable copies the reference, not element storage.

## Mutation Model

- `arr[idx] = value` mutates in place.
- Aliases observe the mutation.

## Bounds Behavior

- Every read/write performs a runtime bounds check: `0 <= idx < len(arr)`.
- Bounds failure traps (WebAssembly trap in M2), with no language-level recovery mechanism yet.
- Negative indexes are invalid and trap due to bounds checks (no Lua-style negative indexing).

## Initial Wasm-GC Representation

M2 uses typed Wasm GC arrays directly as the backing store, with one runtime array type per language element type:

- `[i32]` -> `(array (mut i32))`
- `[i64]` -> `(array (mut i64))`
- `[f32]` -> `(array (mut f32))`
- `[f64]` -> `(array (mut f64))`
- `[bool]` -> `(array (mut i32))` (encoded as `0`/`1` initially)
- nested arrays `[U]` -> `(array (mut (ref null $arr_U)))`

Planned lowering primitives:

- allocation: `array.new_fixed` for literals
- length: `array.len`
- read: `array.get` (or `array.get_s/u` if needed by representation choice)
- write: `array.set`

Because this is the first `wasm-gc` milestone, codegen may keep scalar-only fallback disabled for array programs and emit a clear diagnostic if `wasm-gc` target support is unavailable.

## Fallback Plan

If the selected backend/validator path cannot accept `wasm-gc` yet, compiler behavior is:

1. detect array usage during lowering
2. emit a deterministic "arrays require wasm-gc target support" diagnostic
3. fail compilation for that unit

No hidden fallback to linear-memory emulation is included in M2; that would blur the representation contract and create dual semantics too early.

## IR and Codegen Shape Implications

Frontend/HIR needs explicit constructs for:

- array literal
- array get
- array set
- len builtin call (or dedicated node lowered from builtin)

IR should gain explicit array operations rather than encoding them as opaque calls so the verifier and Wasm lowering can validate operand/result types.

## Examples

```lua
function sum3(a: [i32]): i32
  return a[0] + a[1] + a[2]
end

function mk(): [i32]
  local xs: [i32] = [1, 2, 3]
  xs[1] = 7
  return xs
end
```

## Non-Goals for This Milestone

- compatibility with Lua table/array semantics
- dynamic element typing
- copy-on-write arrays
- custom panic/error objects on bounds failures

M2 is strictly about establishing a typed heap-backed value and validating the `wasm-gc` path under controlled semantics.
