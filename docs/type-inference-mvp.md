# Type Inference MVP Guide

This guide describes what Waluau currently infers automatically, what still requires explicit annotations, and how to resolve common inference failures.

Design source of truth: [`docs/design/0005-type-inference-mvp.md`](design/0005-type-inference-mvp.md).

## What Is Inferred

- local variables with initializer expressions
- non-empty array literal element types
- expression types in typed contexts
- return types for non-recursive functions

## What Is Not Inferred

- function parameter types
- recursive function return types
- generic type arguments at call sites
- empty array literal element types without context

## Successful Inference Examples

```waluau
function add_one(x: i32)
    local y = x + 1
    return y
end
```

`y` is inferred as `i32`, and `add_one`'s return type is inferred as `i32`.

```waluau
function pick(flag: bool)
    local values = {1, 2, 3}
    return if flag then values[0] else values[1]
end
```

`values` is inferred as `{i32}` and the function return type is inferred as `i32`.

## Common Inference Failures

### Missing context for empty arrays

```waluau
function bad(): i32
    local xs = {}
    return #xs
end
```

- Diagnostic code: `inference/missing-context`
- Fix: add an explicit element type:

```waluau
local xs: {i32} = {}
```

### Conflicting return branches

```waluau
function bad(flag: bool)
    if flag then
        return 1
    end
    return true
end
```

- Diagnostic code: `inference/conflict`
- Fix: make branch return types match, or cast explicitly.

### Ambiguous numeric operations

```waluau
function bad(x: i64, y: f64)
    return x + y
end
```

- Diagnostic code: `inference/ambiguous`
- Fix: cast to the intended common numeric type.

### Unsupported recursive return inference

```waluau
function fact(n: i32)
    if n == 0 then
        return 1
    end
    return n * fact(n - 1)
end
```

- Diagnostic code: `inference/unsupported`
- Fix: add an explicit return type annotation:

```waluau
function fact(n: i32): i32
```

## Diagnostic Contract

Inference failures are expected to provide:

- stable diagnostic codes:
  - `inference/ambiguous`
  - `inference/conflict`
  - `inference/unsupported`
  - `inference/missing-context`
- diagnostic category
- source span
- actionable next step
