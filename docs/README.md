# Documentation

This directory is intentionally small. Documentation here should explain only
durable, non-obvious concepts that cannot be understood more reliably from the
code or tests.

Use the repository's authoritative sources instead of maintaining parallel
status documents:

- Compiler and language behavior: crate tests, fixtures, and `conformance/`.
- Conformance runner directives: `conformance/README.md`.
- Compiler structure and runtime representations: the implementation under
  `crates/`, especially `waluau-driver`, `waluau-ir`, and
  `waluau-codegen-wasm`.
- Planned work, known gaps, and priorities: beads (`bd ready`, `bd list`, and
  `bd show`).

Before adding a file, make sure its information will remain valid when features,
priorities, and test coverage change.
