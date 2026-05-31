# 0010: Promise-based Async ABI for Wasm

## Status

Proposed.

## Goal

Define the ABI and runtime contract for integrating Waluau async execution with
JavaScript `Promise`s in WebAssembly builds.

The design must:

- work with the standardized JavaScript Promise Integration API (JSPI)
- preserve ordinary Wasm import and export signatures
- define how errors propagate across suspension points
- define cancellation semantics without pretending native `Promise`s are cancellable
- remain compatible with the current `externref` string ABI and `wasm-gc` array usage

## Current Baseline

Today, Waluau's browser-facing Wasm ABI has these properties:

- strings cross the boundary as `externref`-backed JavaScript strings
- arrays use `wasm-gc` array types
- host interop is defined by explicit Wasm imports plus custom metadata sections
- "coroutine" builtins are currently just zero-argument function wrappers, not
  real yielding coroutines

That means the async ABI must be designed around the JavaScript/Wasm boundary,
not around an existing in-language scheduler.

## Non-Goals

- defining source syntax for `async`/`await`
- implementing in-language futures, tasks, or an event loop
- cancellation of CPU-bound Waluau code while it is running synchronously
- non-browser runtimes such as Wasmtime
- exception handling inside Waluau before Wasm exception handling is adopted

## JSPI Boundary Model

Waluau async interop uses JSPI at the JavaScript embedding layer:

- async-capable Wasm exports are wrapped with `WebAssembly.promising`
- promise-returning JavaScript imports are wrapped with `new WebAssembly.Suspending`

This boundary is deliberate. JSPI suspends and resumes execution at the
JavaScript/Wasm boundary; it does not require new Wasm opcodes or a custom
Promise object representation inside Waluau.

### Consequences

- Wasm function types stay identical to their source-level signatures.
- Async behavior is described by metadata, not by hidden Wasm parameters or a
  separate trampoline ABI.
- A Waluau value is never a JavaScript `Promise`. Promises exist only in the
  embedding layer.

## Source-Level Semantic Model

Source syntax is intentionally left open, but the semantic contract is:

- a Waluau async function produces a value `T` in source semantics
- if it reaches a suspending host call, its JavaScript-facing export result
  becomes `Promise<T>` via `WebAssembly.promising`
- a Waluau call to an async host import behaves like a synchronous call inside
  Waluau, but may suspend the surrounding promising export

This means async lowering can target ordinary IR calls for MVP. The async
boundary is carried in ABI metadata rather than a new IR-level promise type.

## ABI Metadata

Add a versioned custom section:

```text
waluau.asyncabi
```

The section payload is UTF-8 JSON for MVP. This keeps the first implementation
easy to inspect from Rust and JavaScript. If size becomes an issue later, the
encoding can switch to a binary format behind a version bump.

Schema:

```json
{
  "version": 1,
  "imports": [
    {
      "module": "env",
      "name": "fetch_text",
      "kind": "suspending",
      "cancellation": "none"
    }
  ],
  "exports": [
    {
      "name": "load_user",
      "kind": "promising"
    }
  ]
}
```

### Import Entries

Each import entry identifies a Wasm import that must be wrapped in
`WebAssembly.Suspending` before instantiation.

Fields:

- `module`: Wasm import module name
- `name`: Wasm import field name
- `kind`: currently always `"suspending"`
- `cancellation`: `"none"` or `"signal"`

### Export Entries

Each export entry identifies a Wasm export that must be wrapped with
`WebAssembly.promising` before it is exposed as the async JavaScript API.

Fields:

- `name`: Wasm export name
- `kind`: currently always `"promising"`

## Host Runtime Contract

The browser runtime performs three steps after compilation:

1. Read `waluau.asyncabi`.
2. Wrap marked imports with `new WebAssembly.Suspending(...)`.
3. Wrap marked exports with `WebAssembly.promising(...)`.

Pseudo-code:

```js
const metadata = readAsyncAbi(wasmBytes)
const wrappedImports = wrapSuspendingImports(importObject, metadata.imports, runtimeContext)
const { instance } = await WebAssembly.instantiate(wasmBytes, wrappedImports)
const exports = wrapPromisingExports(instance.exports, metadata.exports)
```

The raw `instance.exports` object remains available for synchronous exports. The
wrapped async surface is a second object that should be preferred by the
playground and any browser embedding.

## Suspension Semantics

When Waluau calls an imported suspending function:

1. the JavaScript wrapper is invoked with the ordinary Wasm arguments
2. the wrapper calls the user-provided host function
3. the return value is normalized with `Promise.resolve(...)`
4. JSPI suspends the active promising export until the promise settles
5. fulfillment resumes the suspended Waluau frame with the converted result

Because JSPI already normalizes non-promise values through `Promise.resolve`,
host functions may return either a plain value or a `Promise`. Both satisfy the
ABI. The metadata marks which calls are suspension-capable, not which calls must
always suspend.

## Error Propagation

### Import Side

If a suspending host import:

- throws synchronously
- returns a rejected `Promise`
- resolves to a value that cannot be converted to the Wasm result type

then the call is treated as a thrown JavaScript exception at the Wasm import
boundary.

### Waluau Side

In the MVP, Waluau does not catch these exceptions internally. The active Waluau
call unwinds and terminates the surrounding promising export.

### Export Side

For an export wrapped with `WebAssembly.promising`:

- normal completion resolves the returned JavaScript `Promise`
- a Wasm trap rejects the returned `Promise` with `WebAssembly.RuntimeError`
- a propagated JavaScript exception from a suspending import rejects the
  returned `Promise` with that same exception value

This preserves host-side error identity without inventing a parallel Waluau
error box before exception handling exists in the language runtime.

### Misuse Detection

If a suspending import is called without an active promising export on the
stack, the JSPI embedding throws `WebAssembly.SuspendError`. In practice this
means:

- async-capable exports must be called through their promising wrappers
- synchronous exports must not reach suspending imports

The runtime should fail closed here rather than silently falling back to a
blocking or polling behavior.

## Cancellation Semantics

JavaScript `Promise`s are not intrinsically cancellable, so the ABI uses
cooperative cancellation.

### Rule

Cancellation is only observed at suspending host boundaries.

Waluau code that is already running on the CPU continues until it:

- returns normally
- traps
- or reaches another suspending import

### Runtime Model

Each promising export invocation creates an async call context:

```text
AsyncCallContext {
  signal: AbortSignal | null
}
```

The browser embedding may expose a helper API such as:

```js
runAsyncExport("load_user", [id], { signal })
```

The raw `WebAssembly.promising` wrapper does not carry cancellation state by
itself; the Waluau runtime layer owns that context.

### Cancellable Imports

If an import metadata entry declares `"cancellation": "signal"`, the JS wrapper
invokes the user host function as:

```js
hostFunction(signal, ...wasmArgs)
```

The hidden `signal` argument is injected by the wrapper and is not part of the
Wasm type signature.

The wrapper must race the host result against abort:

- if `signal` aborts before settlement, reject with an `AbortError`
- if the host promise settles first, use that result

### Semantics Seen by Waluau

Waluau does not observe a distinct cancellation result in MVP. Aborts propagate
like any other thrown host exception and reject the outer JavaScript `Promise`.

This keeps the first ABI simple and honest:

- cancellation is explicit
- cancellation is cooperative
- cancellation is not mistaken for preemption

## Type Compatibility

Promise settlement values use the same JS/Wasm coercions as synchronous calls.
JSPI changes suspension behavior, not value conversion rules.

### Scalars

- `i32`, `f32`, `f64`, and `bool` use the existing number/bool conversions
- `i64`/`u64` continue to use JavaScript `BigInt`

### Strings

Strings continue to cross as `externref` JavaScript strings. Suspending and
resuming does not require copying or re-encoding them.

### Wasm GC Values

`wasm-gc` arrays and future GC references remain live in the Wasm store across
suspension. The async ABI must not serialize them through linear memory or JSON.

If a GC value is returned from a promising export, JavaScript receives the same
kind of value it would receive from a synchronous export: an exported Wasm GC
object as defined by the JavaScript embedding.

### Multi-Value Returns

If a promising export returns multiple Wasm values, the resolved JavaScript
value is an array, matching the JSPI export contract.

## Why Metadata Instead of Hidden Parameters

Hidden ABI parameters for async context were rejected for MVP because they would:

- change every async-capable function type
- complicate direct calls between Waluau functions
- leak embedding concerns into the core IR and verifier

Metadata plus JSPI wrappers keep async concerns at the boundary where JSPI
already operates.

## Capability Detection and Fallback

The browser runtime should feature-detect JSPI before exposing async exports:

```js
const hasJspi =
  typeof WebAssembly.promising === "function" &&
  typeof WebAssembly.Suspending === "function"
```

If JSPI is unavailable, async exports should fail with a clear runtime error.
Silent degradation is the wrong default because it would convert a typed async
contract into timing-dependent failure.

An Asyncify-based fallback can be added later as a separate backend choice. That
would be a runtime/codegen extension, not a change to this ABI contract.

## Implementation Plan

1. Add `waluau.asyncabi` metadata emission and parsing.
2. Mark async-capable imports and exports during codegen/linking.
3. Add browser-side wrappers for `Suspending` imports and `promising` exports.
4. Add a small runtime helper for `AbortSignal`-based invocation.
5. Add tests for resolution, rejection, misuse, and cancellation.

## Test Matrix

- metadata round-trip for `waluau.asyncabi`
- sync host value returned through a suspending import
- promise-returning host import suspends and resumes correctly
- rejected import promise rejects the outer export promise
- synchronous throw from import rejects the outer export promise
- calling a suspending import from an unwrapped export surfaces `SuspendError`
- abort before host promise settles rejects with `AbortError`
- strings survive suspend/resume without conversion changes
- `wasm-gc` array values survive suspend/resume without serialization

