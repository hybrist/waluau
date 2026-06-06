# 0010: `unknown` and anyref boxing

## Status

Accepted.

## Goal

Give waluau a way to carry dynamically-typed values through the Wasm backend so
that features crossing a statically-unknown boundary (notably typed
`coroutine.yield`/`resume`, see [0007](0007-coroutines.md)) have a value
representation to build on.

## Source Semantics

A new primitive type is exposed:

```
unknown
```

`unknown` is the type of a value whose static type is not known at compile time.

- **Boxing is implicit.** Any value coerces into `unknown` (it "boxes"). For
  example `local boxed: unknown = x` or passing an argument to an `unknown`
  parameter.
- **Unboxing is explicit.** Recovering a concrete type from an `unknown` value
  requires an explicit cast: `boxed::i32`. Implicitly assigning an `unknown` to
  a concrete type is a type error, because the runtime type is not statically
  known.

An unbox cast traps at runtime if the boxed value does not hold the requested
type.

## Wasm Lowering

`unknown` lowers to `anyref` (`(ref null any)`). Every heap reference and boxed
primitive is a subtype of `any`, so anyref can hold any value, be passed through
function calls, and be stored in struct fields and array elements.

Boxing strategy per primitive:

| source type   | boxed representation        | box op        | unbox op                  |
| ------------- | --------------------------- | ------------- | ------------------------- |
| `i32` / `u32` | `i31ref`                    | `ref.i31`     | `ref.cast (ref i31)` + `i31.get_s` |
| `bool`        | `i31ref` (0/1)              | `ref.i31`     | `ref.cast (ref i31)` + `i31.get_s` |
| `f64`         | `(struct (field f64))`      | `struct.new`  | `ref.cast` + `struct.get` |

`i31ref` holds 31 bits, matching the design's "i31ref for small integers"
intent; values outside the 31-bit range are truncated. `f64` does not fit in an
i31, so it is wrapped in a dedicated `$boxed_f64` struct type that sits with the
always-present closure GC types in the type section.

Boxing/unboxing is realised through the existing IR `Cast` instruction: the
type checker's coercion rules emit a `Cast { from, to: unknown }` (box) or the
explicit-cast path emits `Cast { from: unknown, to }` (unbox), and the Wasm
backend lowers those in `emit_box` / `emit_unbox`.

## Non-Goals / Future Work

- Boxing for `i64`, `u64`, and `f32` is not yet implemented (each would need a
  boxed struct type analogous to `$boxed_f64`); the backend reports a clear
  diagnostic for these.
- Reference types (`string`, `bytes`, arrays, records, functions, `thread`) are
  already heap references and subtypes of `any`; boxing them as `unknown` is a
  natural follow-up but not wired up here.
- A runtime type-test operator (e.g. `is`) for safe unboxing.
