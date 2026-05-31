# 0007: Coroutines

## Status

Accepted.

## Goal

Define the coroutine model, IR representation, and Wasm codegen strategy for
`coroutine_create`, `coroutine_resume`, `coroutine_yield`, and `coroutine_close`.

The design must:

- give coroutines a first-class type distinct from plain functions
- preserve yield-point structure in the IR so multiple Wasm backends can target the same IR
- support a CPS-based Wasm backend today
- make a future stack-switching Wasm backend a codegen-only addition with no IR changes

## Source Semantics

### Coroutine Type

A coroutine is distinct from a function. The source type is:

```
thread
```

The type carries no yield-type parameter. All yield and resume values are `i32`.
Typed generic yields require language-level support for `unknown`/`any` or a
higher-kinded type — tracked separately (see Non-Goals).

### Builtins

```
coroutine_create(f: () -> i32): thread
coroutine_resume(co: thread): (bool, i32)
coroutine_yield(value: i32): ()
coroutine_close(co: thread): bool
```

- `coroutine_create` wraps a zero-argument, `i32`-returning function as a
  coroutine. The function body may call `coroutine_yield`.
- `coroutine_resume` advances the coroutine to the next yield or return, returning
  `(true, value)` where `value` is the yielded or returned `i32`, or `(false, 0)`
  if the coroutine errored or is already dead.
- `coroutine_yield` suspends the currently running coroutine and delivers the
  `i32` value to the caller of `coroutine_resume`. `coroutine_yield` is valid in
  any function called (directly or transitively) while a coroutine is running;
  whether a yield is legal is a runtime check — the static type system only
  verifies that the argument is `i32`.
- `coroutine_close` terminates a suspended or dead coroutine, transitioning it to
  the dead state. Returns `true` if the coroutine was suspended (closed cleanly)
  or was already dead; returns `false` if it was in an error state.

Calling `coroutine_resume` on a dead coroutine returns `(false, 0)`.

### Example

```lua
function make_counter(): thread
    return coroutine_create(function(): i32
        coroutine_yield(1)
        coroutine_yield(2)
        return 3
    end)
end

function run(): i32
    local co: thread = make_counter()
    local ok1: bool
    local a: i32
    ok1, a = coroutine_resume(co)   -- true, 1
    local ok2: bool
    local b: i32
    ok2, b = coroutine_resume(co)   -- true, 2
    local ok3: bool
    local c: i32
    ok3, c = coroutine_resume(co)   -- true, 3  (body returned)
    return a + b + c                -- 6
end
```

## IR Model

### Type Extension

Add a `Thread` variant to the IR type system:

```rust
pub enum Type {
    // existing variants ...
    Thread,
}
```

`Thread` implies `i32` yield/resume values; no type parameter is stored. This
is distinct from `Function { params: [], return_type: I32 }` — a plain function
cannot be used where a `thread` is expected without an explicit
`CoroutineCreate`.

### New Instructions

```rust
pub enum Instruction {
    // existing variants ...

    /// Create a coroutine from a zero-argument, i32-returning function value.
    /// result type: Thread
    CoroutineCreate {
        func: ValueId,
    },

    /// Advance the coroutine to the next yield or return.
    /// Produces two values: ok (bool) and value (i32).
    /// ok = false and value = 0 when the coroutine is dead or errored.
    CoroutineResume {
        coroutine: ValueId,
    },

    /// Suspend the current coroutine and deliver an i32 value to its resumer.
    /// result type: () (control returns when the coroutine is next resumed)
    CoroutineYield {
        value: ValueId,
    },

    /// Transition a suspended or dead coroutine to the dead state.
    /// result type: bool (true = closed cleanly or already dead; false = was errored)
    CoroutineClose {
        coroutine: ValueId,
    },
}
```

`CoroutineYield` is a mid-block instruction, not a terminator. Control
conceptually returns to the yield point when the coroutine is next resumed.

`CoroutineResume` produces two values. The IR represents this as a two-result
instruction; callers extract each component via `ExtractValue` (index 0 = `bool`,
index 1 = `i32`) or an equivalent multi-value mechanism.

### Verifier Rules

- `CoroutineCreate`: `func` must have type `Function { params: [], return_type: I32 }`;
  result type is `Thread`.
- `CoroutineResume`: `coroutine` must have type `Thread`; result is a two-value
  `(Bool, I32)`.
- `CoroutineYield`: `value` must have type `I32`; result type is `()`. No static
  context constraint — whether a coroutine is on the call stack is a runtime check.
- `CoroutineClose`: `coroutine` must have type `Thread`; result type is `Bool`.

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
{ tag: i32, yielded_value: i32, continuation: funcref, ...captures }
```

- `tag` tracks whether the coroutine is `suspended` (0), `finished` (1), or
  `error` (2).
- `yielded_value` holds the `i32` written by the most recent `CoroutineYield` or
  the final return value of the body; `0` when `tag` is `error`.
- `continuation` is the funcref to call on the next `resume`. It is updated at
  each yield point and cleared when the coroutine finishes or errors.
- Captures from the enclosing scope are stored in the state struct alongside
  `tag`, `yielded_value`, and `continuation`.

Lowering steps:

1. Identify all `CoroutineYield` points in the coroutine body.
2. Split the body into segments at each yield: segment 0 is everything up to the
   first yield, segment N is from yield N to yield N+1 (or the return).
3. Emit each segment as a separate Wasm function that takes the state struct
   pointer as an argument; on yield it stores the `i32` into `yielded_value`,
   updates `continuation`, and sets `tag = suspended`; on normal return it stores
   the return value into `yielded_value` and sets `tag = finished`; on error it
   sets `tag = error` and `yielded_value = 0`.
4. `CoroutineCreate` allocates the state struct, sets `tag = suspended`, and sets
   `continuation` to the first segment function.
5. `CoroutineResume` checks `tag`: if `finished` or `error` returns `(false, 0)`;
   if `suspended` calls `continuation(state)`, then returns `(tag != error,
   yielded_value)`.
6. `CoroutineClose` sets `tag = finished` and zeros the `continuation` funcref.

### Stack-Switching Backend (future)

When the Wasm stack-switching proposal is available, the same IR can be lowered
directly:

- `CoroutineCreate` → `cont.new $coroutine_type (ref.func $body)`
- `CoroutineResume` → `cont.resume $yield_tag` (result includes the yielded `i32`)
- `CoroutineYield` → `suspend $yield_tag` (passes the `i32` to the resumer)
- `CoroutineClose` → `cont.cancel` (or equivalent once the proposal stabilises)

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
  `CoroutineResume`, `CoroutineYield`, `CoroutineClose`). The IR was designed with
  this mapping in mind.

## Diagnostics

Required stable diagnostics:

- `coroutine_create` argument is not a zero-argument `i32`-returning function (static)
- `coroutine_yield` argument is not `i32` (static)
- `coroutine_resume` argument is not a `thread` (static)
- `coroutine_close` argument is not a `thread` (static)
- `coroutine_yield` called outside a coroutine context (runtime)
- `coroutine_resume` on a dead coroutine returns `(false, 0)` (runtime, not a trap)

## Test Matrix

- HIR type checking:
  - `coroutine_create` round-trip: `() -> i32` function → `thread`
  - `coroutine_resume` produces `(bool, i32)`
  - `coroutine_yield` accepted inside coroutine body
  - `coroutine_yield` with non-`i32` argument rejected
  - `coroutine_create` rejected for non-zero-arg function
  - `coroutine_create` rejected for function returning non-`i32`
- IR verifier:
  - `CoroutineCreate` type rules
  - `CoroutineResume` type rules
  - `CoroutineYield` value type rule (must be `I32`); no context restriction
  - `CoroutineClose` type rules
- codegen / driver e2e (CPS backend):
  - single yield then return
  - multiple sequential yields
  - coroutine passed between functions
  - nested coroutines
  - resume after completion returns `(false, 0)`
  - close on suspended coroutine returns `true`
  - close on dead coroutine returns `true`
  - resume after close returns `(false, 0)`

## Non-Goals

- Generic yield/resume types — requires `any`/`unknown` or type-parameter support;
  tracked separately once the type system gains a catch-all type
- Resume arguments / arguments on first create — type-safe bidirectional values;
  tracked separately (passing initial args on first resume vs on create has
  significant type-system implications and is a non-starter if first-resume args
  differ from subsequent resumes)
- `coroutine_wrap` convenience wrapper
- `coroutine_status` / `coroutine_running` / `coroutine_isyieldable` query APIs
- Symmetric coroutines / first-class continuations
- Stack-switching backend implementation (tracked separately)
