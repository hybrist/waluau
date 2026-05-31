# Generic Functions MVP Guide

This guide documents how to use generic functions in the current MVP.

For full design rationale and semantics, see [0009](./design/0009-generic-functions-mvp.md).

## Quick Start

Generic function declarations use explicit type parameters:

```lua
function id<T>(value: T): T
  return value
end

function main(): i32
  return id<i32>(41)
end
```

Multiple type parameters are supported:

```lua
function choose<T>(condition: bool, a: T, b: T): T
  if condition then
    return a
  else
    return b
  end
end

function main(): i32
  return choose<i32>(true, 1, 2)
end
```

Generic function expressions are also supported:

```lua
local id = function<T>(value: T): T
  return value
end

function main(): i32
  return id<i32>(1)
end
```

## Rules You Must Follow

- Type arguments are required at each generic call site.
- Type argument count must exactly match the generic parameter count.
- Generic functions must have explicit return type annotations.
- Uninstantiated generic function values are not first-class in this MVP.

## Unsupported in MVP

The following remain intentionally unsupported:

- type-argument inference (`id(1)` for `id<T>`)
- constraints/bounds on type parameters
- default type parameters
- generic types like `Array<T>` and similar generic type constructors
- polymorphic function value types (`forall<T>...`) and storing/passing uninstantiated generic functions

## Common Diagnostics

- `generic/missing-type-args`: generic function call omitted explicit type arguments.
- `generic/type-arg-count`: wrong number of type arguments.
- `generic/extra-type-args`: type arguments used on a non-generic callee.
- `generic/uninstantiated-value`: generic function used as a value without instantiation.
- `generic/missing-return-type`: generic function declaration/expression is missing an explicit return type.
- `generic/unsupported-type`: unsupported generic type syntax like `Array<i32>`.

## Where Behavior Is Locked by Tests

Parser and syntax coverage:

- [`crates/waluau-parser/src/lib.rs`](../crates/waluau-parser/src/lib.rs)
  - `parses_generic_function_declaration`
  - `parses_generic_call_with_type_arguments`
  - `rejects_generic_type_annotation`
  - `<` comparison parsing guards (`allows_less_than_comparison_not_confused_with_generics`)

Typechecker generic behavior:

- [`crates/waluau-hir/tests/generics.rs`](../crates/waluau-hir/tests/generics.rs)
  - success paths (`identity`, `choose`)
  - missing type args
  - wrong type-arg count
  - uninstantiated generic values

Diagnostic code/category stability:

- [`crates/waluau-hir/tests/inference_codes.rs`](../crates/waluau-hir/tests/inference_codes.rs)
  - `generic/unsupported-type` and related diagnostic expectations

Lowering/monomorphization behavior:

- [`crates/waluau-ir/src/lib.rs`](../crates/waluau-ir/src/lib.rs)
  - `monomorphizes_generic_calls_once_per_type_arguments`
  - `rejects_cross_specialization_recursive_generics`

Runtime execution path:

- [`crates/waluau-driver/src/lib.rs`](../crates/waluau-driver/src/lib.rs)
  - `executes_generic_specializations`

## Contributor Notes

When extending generic behavior, update both:

- this guide (for externally visible behavior), and
- [0009 design doc](./design/0009-generic-functions-mvp.md) (for semantic/design decisions).

If a new limitation or failure mode is found, add a dedicated diagnostic code and
regression test before broadening behavior.
