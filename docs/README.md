# Documentation

This directory is intentionally small. Documentation here should explain only
durable, non-obvious concepts that cannot be understood more reliably from the
code or tests.

[`platform-target.md`](platform-target.md) is the exception that proves the rule:
which platform this project targets is a decision, not a fact recoverable from
the code, so it is written down once and referenced everywhere else.

Use the repository's authoritative sources instead of maintaining parallel
status documents:

- Platform target, workload placement, and testing strategy: `platform-target.md`.
- Compiler and language behavior: crate tests, fixtures, and `conformance/`.
- Conformance runner directives: `conformance/README.md`.
- Compiler structure and runtime representations: the implementation under
  `crates/`, especially `waluau-driver`, `waluau-ir`, and
  `waluau-codegen-wasm`.
- Planned work, known gaps, and priorities: beads (`bd ready`, `bd list`, and
  `bd show`).

Before adding a file, make sure its information will remain valid when features,
priorities, and test coverage change.
