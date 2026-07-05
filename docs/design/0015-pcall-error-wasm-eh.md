# 0015: `pcall` and Lua Errors via Wasm Exception Handling

## Status

Implemented as an MVP.

## Problem

Luau programs use `pcall` to turn a thrown Lua error into normal values:

```lua
local ok, value_or_error = pcall(f, ...)
```

Waluau previously lowered assertion failures to a printed message followed by
`unreachable`, so there was no recoverable error value for `pcall` to catch.
This blocked many Luau conformance chunks and made `error(...)`/`assert(...,
msg)` impossible to model faithfully.

## Runtime Representation

The MVP represents Lua errors as Wasm exceptions with a single module-local tag:

```wat
(tag $lua_error (param anyref))
```

The payload is `unknown` (`anyref`). String errors are converted with
`any.convert_extern` before `throw`, and callers that need the message cast the
second pcall result back to `string`.

`pcall` lowers to a `ProtectedCall` IR instruction and returns:

```lua
(bool, unknown)
```

The success payload is the callee's scalar return boxed into `unknown`. Unit
returns become `null anyref`. Failure payloads are the caught exception payload.

This intentionally mirrors the existing coroutine fallback shape
`(bool, unknown)` rather than introducing a pcall-specific tagged union in the
first implementation.

## Lowering

`assert(false, msg)` and `error(msg)` emit a `Throw` IR instruction and an
unreachable terminator. `assert(cond)` still synthesizes the current
`Assertion failed: <expr> at <file>:<line>` message.

`pcall(f, ...)` type-checks the protected function and arguments statically, then
emits:

1. `ok = false`
2. `try_table (result anyref) (catch $lua_error outer)`
3. Call the function through the same closure representation as `CallValue`
4. Box the scalar success value into `unknown`
5. Set `ok = true`
6. Store the block result into the second multi-value slot

The tag section is emitted before globals, matching wasmparser's exception
handling section order.

## Scope and Limits

Supported now:

- `pcall(function() return scalar end)` as `(true, unknown)`
- `pcall(function() assert(false, "message") end)` as `(false, unknown)`
- `error(message)` and `assert(condition, message)` for string messages
- top-level conformance execution, because catching happens inside Wasm rather
  than by calling back into a JavaScript host wrapper during instantiation

Not yet supported:

- Multiple success payload values from `pcall`
- Catching raw Wasm traps such as integer divide-by-zero
- `xpcall`
- Dynamic truthiness for non-bool `assert` conditions
- Lua's `level` stack-prefix behavior beyond accepting the second `error`
  argument syntactically

## Type Story

The MVP keeps `pcall` in the typed subset as:

```lua
pcall<T...>(f: (...) -> T, ...): (bool, unknown)
```

Callers cast the payload after checking `ok`. A later version can generalize this
to a tagged result, for example `Error(string) | Ok(unknown)`, but that would
need a better multi-payload tagged-union story first.
