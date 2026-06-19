# Imported Luau conformance tests

This directory vendors the upstream **Luau** conformance test suite so Waluau can
be measured against the behaviour of the reference language.

## Provenance

- **Source:** [`luau-lang/luau`](https://github.com/luau-lang/luau),
  `tests/conformance/*.luau`
- **Commit:** `86d2a9dcd7cef396b73b1585371723e169e69a41`
- **License:** MIT — see [`LICENSE`](./LICENSE) (copied verbatim from the
  upstream `LICENSE.txt`). Copyright © Roblox Corporation and Lua.org, PUC-Rio.

Each test is the upstream `tests/conformance/<name>.luau` file copied **verbatim**,
with two mechanical changes:

1. The file extension is changed from `.luau` to `.walu` so the conformance
   runner (which globs `**/*.walu`) discovers it. The contents are otherwise
   unmodified.
2. A short header comment is prepended recording where the file came from and
   marking it `-- conformance: pending` (see below).

## Why every file is `pending`

These are full, untyped, dynamically-typed Lua/Luau programs that lean on the
complete reference runtime: globals such as `print`, `select`, `tostring`,
`pcall`/`error`, the `string`/`table`/`math`/`coroutine`/`buffer` libraries,
metatables, varargs, and so on. Waluau today compiles a small, statically-typed
subset of Luau to WebAssembly, so **none of these files compile yet** — every one
currently fails (most at the parser, a few via deeper recursion limits).

The runner's [`pending`](../README.md#test-kinds) directive is exactly the tool
for this situation: a pending test is one that *should* pass eventually but does
not yet, and the runner only verifies that it currently fails (it does not care
how). When Waluau grows enough of the language for one of these files to compile
and instantiate cleanly, that file's `pending` test will go red — that is the
signal to remove the `-- conformance: pending` directive from its header (and, if
needed, adapt the test to Waluau's harness).

So this directory doubles as a backlog: the set of still-pending files is a
running measure of how much of the Luau language Waluau does not yet implement.

## Updating the import

To re-sync against a newer upstream revision:

1. Clone `luau-lang/luau` and copy `tests/conformance/*.luau` here, renaming each
   to `*.walu` and prepending the provenance + `pending` header.
2. Update the commit SHA above and in each file's header.
3. Re-run the suite (`pnpm --filter conformance-runner test:browser`). Any file
   that now passes should have its `pending` directive removed.

Do not hand-edit the bodies of these files: keeping them byte-for-byte identical
to upstream (modulo the header) is what makes them a meaningful conformance
reference.
