# Imported Luau conformance tests

This directory vendors the upstream **Luau** conformance suite so Waluau can
measure the behavior it shares with the reference language and identify the
places where its typed browser/Wasm-GC language differs deliberately.

## Provenance

- **Source:** [`luau-lang/luau`](https://github.com/luau-lang/luau),
  `tests/conformance/*.luau`
- **Commit:** `86d2a9dcd7cef396b73b1585371723e169e69a41`
- **License:** MIT — see [`LICENSE`](./LICENSE), copied verbatim from the
  upstream `LICENSE.txt`. Copyright © Roblox Corporation and Lua.org, PUC-Rio.

Most test bodies are upstream text with only a `.walu` extension and a
provenance header. Large upstream files may be split into numbered chunks so an
independent passing section is not hidden behind an unrelated failure. A split
header records its source interval and any upstream helper setup repeated to
make the chunk runnable. A few explicitly named `*.patched.walu` companions
adapt useful coverage to Waluau; their edits are marked in the file and do not
replace the verbatim chunks.

Do not silently edit an imported assertion. Preserve source order, keep every
removed interval represented by another chunk, and repeat only setup needed for
standalone execution.

## Enabled and pending chunks

A chunk without `-- conformance: pending` must compile and pass in the browser.
A pending chunk is expected to fail today; if it starts passing, the runner
fails so the marker cannot become stale.

Pending does **not** always mean “planned feature.” Some chunks expose a bounded
language or library gap, while others test deliberate differences such as the
typed coroutine API, runtime source compilation, native/JIT machinery, or
sparse Lua tables. [`DEVIATIONS.md`](./DEVIATIONS.md) records those broad
categories, their semantic impact, the exact affected chunk sets, and related
beads.

Splitting can increase both the total and pending chunk counts: one coarse
pending program may become several focused pending chunks plus newly enabled
chunks. The meaningful progress metric is enabled upstream coverage, not a
monotonically decreasing pending count.

Reproduce the current directory-level counts from the repository root with:

```sh
find conformance/luau -maxdepth 1 -name '*.walu' -print | wc -l
rg -l '^-- conformance: pending$' conformance/luau -g '*.walu' | wc -l
```

The difference is the enabled count. The full behavior check is:

```sh
node conformance/luau/check-pending-inventory.mjs
pnpm --filter conformance-runner test:browser
```

The inventory check pins the exact snapshot, verifies every pending filename is
covered by the family mapping in [`DEVIATIONS.md`](./DEVIATIONS.md), checks the
compact intentional-execution sets, and keeps `native.53` as the sole exact-name
VM/JIT exclusion. The browser suite then executes every other pending chunk as
an inverse test: if any starts passing, the suite fails until it is enabled or
split.

## Working a pending chunk

1. Read its provenance header and find its category in
   [`DEVIATIONS.md`](./DEVIATIONS.md).
2. If it is a fixable gap, use the linked bead or create a discovered-from bead
   under `waluau-q7qg` before changing code.
3. If independent assertions already pass, split at upstream assertion
   boundaries. Do not rewrite them merely to make the compiler accept them.
4. Remove `-- conformance: pending` only when the complete resulting chunk
   passes in the browser.

## Updating the import

To re-sync against a newer upstream revision:

1. Import the new upstream sources and license, preserving the distinction
   between verbatim chunks and explicitly adapted companions.
2. Update the commit SHA above and in every imported header.
3. Reapply existing provenance-preserving splits against the new source ranges.
4. Recompute the inventory in [`DEVIATIONS.md`](./DEVIATIONS.md), then run the
   browser suite. Any pending chunk that now passes must be enabled or split so
   its passing coverage is visible.
