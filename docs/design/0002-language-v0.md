# 0002: Language V0

## Status

Accepted.

## Goal

Define the first supported language slice tightly enough that frontend, CFG, SSA, and scalar Wasm code generation can be built without dragging in runtime-heavy Lua features.

## Language Model

The language is Lua-like in syntax, but not Lua-compatible in semantics.

The compiler should reject unsupported dynamic behavior explicitly rather than partially inheriting Lua rules.

V0 surface syntax follows Lua/Luau spellings: `function`, `local`, return type annotations with `:`, and logical operators `and` / `or`. Alternate spellings (`fn`, `let`, `&&`, `||`) are rejected with diagnostics.

## Included in V0

- integer literals
- boolean literals
- typed local declarations
- typed function parameters
- typed function returns
- function types: `(T1, T2) -> R`
- variable references
- unary operators: numeric negation and boolean `not`
- binary arithmetic and comparison operators
- `if` / `elseif` / `else`
- `while`
- assignment to locals
- direct function calls
- function expressions and lexical closures
- `return`

Example:

```lua
function sum_to(n: i32): i32
  local i: i32 = 0
  local acc: i32 = 0
  while i < n do
    acc = acc + i
    i = i + 1
  end
  return acc
end
```

## Excluded from V0

- comments (`--` line comments and `--[[...]]` block comments)
- `nil`
- strings
- arrays
- tables
- multiple returns
- varargs
- modules
- metatables
- dynamic typing escapes
- implicit truthiness rules

## Types

V0 supports:

- `u32`
- `u64`
- `i32`
- `i64`
- `f32`
- `f64`
- `bool`
- first-class function values (`(T1, T2) -> R`)

For source convenience, `number` is accepted as an alias for `f64`. It is not a separate semantic type.

## Typing Rules

- All locals require explicit type annotations.
- All function parameters require explicit type annotations.
- All functions require explicit return types.
- Function literals use the same typed signature syntax as declarations:
  - `function(x: i32): i32 ... end`
  - optional local self-name for recursion: `function self(x: i32): i32 ... end`
- Branch conditions must have type `bool`.
- Assignment allows implicit numeric widening only when the destination can represent the full source range.
- Return expressions follow the same implicit numeric widening rule.

Example of a required error:

```lua
function bad(x: i32): i32
  if x then
    return 1
  end
  return 0
end
```

The condition `x` must be rejected because `i32` is not `bool`.

Numeric operations use the smallest predictable implicit conversion rule: keep exact scalar agreement by default, allow only non-lossy widening when a common type exists, and require explicit casts for narrowing or lossy conversions. For example, `i32 + i64` widens to `i64`, `i32 + f64` widens to `f64`, and `i64 + f64` must be rejected unless one side is rewritten with `::`.

Explicit numeric casts use postfix `expr :: Type` syntax. Casts are only valid between numeric scalar types.

## Semantics

- `if` and `while` consume boolean conditions only.
- Arithmetic is statically typed, not dynamically coerced.
- Function names resolve statically.
- Closures capture lexically scoped locals from outer scopes.
- Expression statements are only valid for calls.
- Unsupported Lua constructs should produce clear diagnostics.

## Non-Goals

V0 is not trying to be a useful Lua replacement. It exists to force the compiler through the key architectural steps:

- parsing a real statement language
- name resolution
- type checking
- control-flow lowering
- SSA construction
- code generation

That is enough pressure to validate the architecture without committing to a runtime too early.

## Early Test Programs

Arithmetic:

```lua
function add(a: i32, b: i32): i32
  return a + b
end
```

Branching:

```lua
function max(a: i32, b: i32): i32
  if a > b then
    return a
  else
    return b
  end
end
```

Mutable local merge:

```lua
function f(x: i32): i32
  local y: i32 = 0
  if x > 0 then
    y = x
  end
  return y
end
```
