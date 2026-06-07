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

`waluau-snxo` is expected to add extern inheritance and safe cast narrowing.
Until that lands, generated `.walu` output is intentionally flat and does not
emit inheritance syntax. Parent interface names are preserved in metadata so the
generator can switch to direct inheritance emission later without changing the
IDL fixture.
