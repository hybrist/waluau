# 0007: Coroutines

## Status

Proposed.

## Goal

Define the coroutine model, IR representation, and Wasm codegen strategy for
`coroutine_create`, `coroutine_resume`, and `coroutine_yield`.

The design must:

- give coroutines a first-class type distinct from plain functions
- preserve yield-point structure in the IR so multiple Wasm backends can target the same IR
- support a CPS-based Wasm backend today
- make a future stack-switching Wasm backend a codegen-only addition with no IR changes

## Source Semantics

### Coroutine Type

A coroutine is distinct from a function. The source type syntax is:

```
coroutine<R>
```

where `R` is the yield/return type. A coroutine of type `coroutine<i32>` yields
and returns `i32` values.

### Builtins

```
coroutine_create(f: () -> R): coroutine<R>
coroutine_resume(co: coroutine<R>): R
coroutine_yield(value: R): ()
```

- `coroutine_create` wraps a zero-argument function as a coroutine. The function
  body may call `coroutine_yield`.
- `coroutine_resume` advances the coroutine to the next yield or return,
  producing the yielded/returned value.
- `coroutine_yield` is only valid inside a coroutine body. It suspends the
  coroutine and delivers a value to the caller of `coroutine_resume`.

Calling `coroutine_resume` on a finished coroutine (one that has returned) is a
runtime error.

### Example

```lua
function make_counter(): coroutine<i32>
    return coroutine_create(function(): i32
        coroutine_yield(1)
        coroutine_yield(2)
        return 3
    end)
end

function run(): i32
    local co: coroutine<i32> = make_counter()
    local a: i32 = coroutine_resume(co)  -- 1
    local b: i32 = coroutine_resume(co)  -- 2
    local c: i32 = coroutine_resume(co)  -- 3
    return a + b + c                     -- 6
end
```

## IR Model

### Type Extension

Add a `Coroutine` variant to the IR type system:

```rust
pub enum Type {
    // existing variants ...
    Coroutine { yield_type: Box<Type> },
}
```

`Coroutine { yield_type: T }` and `Function { params: [], return_type: T }` are
distinct types. A plain function cannot be used where a coroutine is expected
without an explicit `coroutine_create`.

### New Instructions

```rust
pub enum Instruction {
    // existing variants ...

    /// Create a coroutine from a zero-argument function value.
    /// result type: Coroutine { yield_type }
    CoroutineCreate {
        func: ValueId,
        yield_type: Type,
    },

    /// Advance the coroutine to the next yield or return.
    /// result type: yield_type
    CoroutineResume {
        coroutine: ValueId,
    },

    /// Suspend the current coroutine and deliver a value to its resumer.
    /// result type: () (no value; control returns when resumed again)
    CoroutineYield {
        value: ValueId,
    },
}
```

`CoroutineYield` is a mid-block instruction, not a terminator. Control
conceptually returns to the yield point when the coroutine is next resumed.

### Verifier Rules

- `CoroutineCreate`: `func` must have type `Function { params: [], return_type: T }`;
  result type is `Coroutine { yield_type: T }`.
- `CoroutineResume`: `coroutine` must have type `Coroutine { yield_type: T }`;
  result type is `T`.
- `CoroutineYield`: `value` must have type `T` where the enclosing function was
  created via `CoroutineCreate` with `yield_type: T`; only valid inside a
  coroutine body.

## Wasm Codegen Strategy

The IR preserves the coroutine body as a single function with explicit
`CoroutineYield` instructions. The Wasm backend chooses a lowering strategy.
This choice is a codegen configuration concern and requires no IR changes.

### Strategy Enum

```rust
pub enum CoroutineBackend {
    /// Lower via CPS transformation. Works on all current Wasm targets.
    Cps,
    /// Emit native stack-switching instructions. Requires the Wasm
    /// stack-switching proposal.
    StackSwitching,
}
```

### CPS Backend (current)

The CPS backend splits the coroutine body at each `CoroutineYield` into
continuation functions, linked by a state struct stored in linear memory or a
Wasm table slot.

State struct layout (per coroutine instance):

```
{ tag: i32, continuation: funcref }
```

- `tag` tracks whether the coroutine is `running`, `suspended`, or `finished`.
- `continuation` is the funcref to call on the next `resume`. It is updated at
  each yield point and cleared on return.

Lowering steps:

1. Identify all `CoroutineYield` points in the coroutine body.
2. Split the body into segments at each yield: segment 0 is everything up to the
   first yield, segment N is from yield N to yield N+1 (or the return).
3. Emit each segment as a separate Wasm function that takes the state struct
   pointer as an argument and updates `continuation` before returning to the
   resumer.
4. `CoroutineCreate` allocates the state struct and sets `continuation` to the
   first segment function.
5. `CoroutineResume` checks `tag`, calls `continuation(state)`, and returns the
   yielded/returned value.

Captures from the enclosing scope are stored in the state struct alongside `tag`
and `continuation`.

### Stack-Switching Backend (future)

When the Wasm stack-switching proposal is available, the same IR can be lowered
directly:

- `CoroutineCreate` → `cont.new $coroutine_type (ref.func $body)`
- `CoroutineResume` → `cont.resume $yield_tag`
- `CoroutineYield` → `suspend $yield_tag`

No structural transformation is required. The coroutine body emits as a single
Wasm function with `suspend` instructions in place of yield points.

### Why this split is the right boundary

CPS transformation is a backend concern because:

- CPS destroys the one-function-with-yield-points structure. Encoding it in the
  IR would make a future stack-switching backend require un-CPS, which is not
  feasible.
- Asyncify (binary instrumentation) is also backend-scoped and similarly does not
  require IR changes, but it adds overhead at every function call, not just at
  yield points.
- Stack-switching maps almost 1:1 onto the IR instructions (`CoroutineCreate`,
  `CoroutineResume`, `CoroutineYield`). The IR was designed with this mapping in
  mind.

## Diagnostics

Required stable diagnostics:

- `coroutine_create` argument is not a zero-argument function
- `coroutine_yield` used outside a coroutine body
- `coroutine_resume` argument is not a coroutine
- type mismatch between yield value and coroutine yield type
- resume called on a finished coroutine (runtime)

## Test Matrix

- HIR type checking:
  - `coroutine_create` round-trip: function → coroutine
  - `coroutine_resume` produces the yield type
  - `coroutine_yield` accepted inside coroutine body
  - `coroutine_yield` rejected outside coroutine body
  - type mismatch on yield value
  - `coroutine_create` rejected for non-zero-arg function
- IR verifier:
  - `CoroutineCreate` type rules
  - `CoroutineResume` type rules
  - `CoroutineYield` context validation
- codegen / driver e2e (CPS backend):
  - single yield then return
  - multiple sequential yields
  - coroutine passed between functions
  - nested coroutines
  - resume after completion (runtime error path)

## Non-Goals

- coroutines with arguments to `resume` (values passed into the yield point)
- multi-value yield
- coroutine status query (`coroutine.status`)
- symmetric coroutines / first-class continuations
- stack-switching backend implementation (tracked separately)
