# DOM Web IDL Extern Generation

This directory contains the first deterministic slice of DOM extern generation.
It intentionally uses a small checked-in Web IDL fixture instead of fetching a
package at generation time.

Regenerate the checked-in output with:

```bash
pnpm generate:dom-externs
```

The generator writes:

- `externs/dom.walu`: Waluau extern type, method, and property declarations.
- `externs/dom.metadata.json`: parsed interface inheritance and emitted/skipped member metadata.
- `externs/dom.diagnostics.txt`: deterministic skip diagnostics for unsupported or filtered IDL.

The generated `.walu` output emits extern inheritance with
`type Child = extern extends Parent`. Parent interface names are also mirrored in
metadata so downstream tooling can inspect the DOM hierarchy without reparsing
the generated source.

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
