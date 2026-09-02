# DOM Web IDL Extern Generation

This directory contains the first deterministic slice of DOM extern generation.
It intentionally uses a small checked-in Web IDL fixture instead of fetching a
package at generation time.

Regenerate the checked-in output with:

```bash
pnpm generate:dom-externs
```

The generator formats `externs/dom.walu` with `waluau fmt` before writing it,
so the checked-in file matches `pnpm format:check`. It uses the binary named by
`WALUAU_BIN`, then a previously built `target/{debug,release}/waluau`, and
otherwise falls back to `cargo run -p waluau-cli`. The generator's tests
(`pnpm test:dom-externs`) run in the Rust CI workflow and compare a fresh
generation against the checked-in output.

The generator writes:

- `externs/dom.walu`: Waluau extern type, method, and property declarations.
- `externs/dom.metadata.json`: parsed interface inheritance and emitted/skipped member metadata.
- `externs/dom.diagnostics.txt`: deterministic skip diagnostics for unsupported or filtered IDL.

The generated `.walu` output emits extern inheritance with
`type Child = extern extends Parent`. Parent interface names are also mirrored in
metadata so downstream tooling can inspect the DOM hierarchy without reparsing
the generated source.

## Overloads and optional parameters

The waluau compiler supports overloaded `declare function` entries (same name,
different parameter types or arity), and the generator leans on that in two
ways:

- Web IDL overloads of one operation (e.g. `fill()` and `fill(Path2D)`) are
  each considered independently; every overload whose types map is emitted,
  so an unrepresentable overload no longer drags down the whole member.
- Trailing `optional` parameters expand into one declaration per emittable
  arity: the required prefix first, then one more declaration per optional
  parameter whose type maps (e.g. `arc(...)` with and without
  `counterclockwise`). Expansion stops at the first optional parameter with
  no extern representation.

Overloads that collapse to the same extern parameter types are emitted once.
At the wasm level each overload becomes its own import under the shared
`Interface.member` name; the playground host bridge forwards arguments
variadically, so one JS implementation serves every arity.

## Union types

Web IDL union types (e.g. `(DOMString or CanvasGradient or CanvasPattern)`) are
untagged/structural and have no general representation in waluau's extern type
system, so unions are skipped by default (see waluau-o4xs). `filter.json`'s
`unionTypeMap` opts specific, exact union shapes into a single-type collapse
where that is sound: keys are the stringified union (without any trailing `?`;
nullability is applied after the lookup), values are the waluau type to emit.
The one entry today collapses the canvas paint-style union to `string`, which
is always valid to assign to `fillStyle`/`strokeStyle`; the getter is loosened
to `string` as well. Before adding entries, check that newly emitted members do
not introduce cross-interface duplicate-name collisions (waluau-1l02).
