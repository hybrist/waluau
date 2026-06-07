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
