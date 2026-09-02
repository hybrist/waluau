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

## Enabled, pending, and out-of-scope chunks

Every chunk is in exactly one of three states:

- **Enabled** — no directive. The chunk must compile and pass in the browser.
- `-- conformance: pending` — a bounded gap someone could fix. Every pending
  family is owned by an open bead.
- `-- conformance: out-of-scope: <slug>[,<slug>]` — a documented deliberate
  difference blocks it, such as runtime source compilation, Luau VM internals,
  the typed coroutine API, or sparse Lua tables. Nobody is expected to make it
  pass. Each slug names a section of [`DEVIATIONS.md`](./DEVIATIONS.md).

The distinction exists so the headline number means something. A chunk that
tests `loadstring` or the native JIT is not backlog; counting it with a real
inference gap made the pending number overstate the addressable work by roughly
two to one.

`-- conformance: untriaged: <reason>` is a **variant of pending**, not a fourth
state. It counts inside the pending total and the checker reports it as a subset,
as in `236 pending (30 untriaged)`.

Reach for `untriaged` when you **cannot tell which of the two buckets a chunk
belongs in** — when the evidence does not settle whether a bounded gap or a
deliberate difference is what actually stops it. Reach for plain `pending` when
you can. The inline reason is required and free text, and it is the whole point:
it must say what the open question is, and name the alternative classification
where the doubt has one, so a later audit finds the doubt with a grep instead of
re-deriving it.

```sh
rg '^-- conformance: untriaged:' conformance/luau -g '*.walu'
```

It defaults toward `pending` on purpose. Over-claiming `out-of-scope` is the
failure mode that makes the split worthless, so a chunk you are unsure about must
never sit in the "won't fix" bucket: `untriaged` wins over `out-of-scope`
whenever there is real doubt.

All three non-enabled states are **inverse-tested**. The browser suite runs them
and requires them to fail today, so a chunk that starts passing turns the suite
red whichever marker it carries and no marker can go stale.

Splitting can increase both the total and the non-enabled counts: one coarse
chunk may become several focused chunks plus newly enabled ones. The meaningful
progress metric is enabled upstream coverage, not a monotonically decreasing
pending count.

Reproduce the current directory-level counts from the repository root with:

```sh
find conformance/luau -maxdepth 1 -name '*.walu' -print | wc -l
rg -l '^-- conformance: (pending$|untriaged:)' conformance/luau -g '*.walu' | wc -l
rg -l '^-- conformance: untriaged:' conformance/luau -g '*.walu' | wc -l
rg -l '^-- conformance: out-of-scope:' conformance/luau -g '*.walu' | wc -l
```

The full behavior check is:

```sh
node conformance/luau/check-pending-inventory.mjs
pnpm --filter conformance-runner test:browser
```

The inventory check pins the whole-directory totals, verifies that every chunk
carries at most one marker, requires every `untriaged` directive to state its
open question, validates every out-of-scope slug against the documented deviation
set, requires every pending family to name an open bead, and keeps `native.53` as
the sole exact-name VM/JIT runner exclusion. The three inventory tables in
[`DEVIATIONS.md`](./DEVIATIONS.md) — out-of-scope by deviation, pending by
family, and the untriaged open questions — are **generated from the chunk
markers** rather than hand-maintained; regenerate them with
`node conformance/luau/check-pending-inventory.mjs --write`.

## Working a chunk

1. Read its provenance header and its marker.
2. Pending: find the owning bead in the generated pending table in
   [`DEVIATIONS.md`](./DEVIATIONS.md), or create a discovered-from bead before
   changing code.
3. Out-of-scope: read the named deviation section first. If you believe the
   chunk is misclassified, recompile it and read its first diagnostic before
   arguing from intent — the classification is evidence-based.
4. Untriaged: the directive states the open question. Settle it, then move the
   chunk to plain `pending` or to `out-of-scope` with a slug.
5. If independent assertions already pass, split at upstream assertion
   boundaries. Do not rewrite them merely to make the compiler accept them.
6. Remove a directive only when the complete resulting chunk passes in the
   browser. Reclassifying between directives requires rerunning the inventory
   check with `--write`.

## Updating the import

To re-sync against a newer upstream revision:

1. Import the new upstream sources and license, preserving the distinction
   between verbatim chunks and explicitly adapted companions.
2. Update the commit SHA above and in every imported header.
3. Reapply existing provenance-preserving splits against the new source ranges.
4. Reclassify the new chunks by compiling each one and reading its first
   diagnostic, then run `check-pending-inventory.mjs --write` and the browser
   suite. Any non-enabled chunk that now passes must be enabled or split so its
   passing coverage is visible.
