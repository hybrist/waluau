# Compiler performance benchmarks

The reference workload is `apps/ante` — a production game (~0.9 MB of Waluau
source, ~55 source files under `apps/ante/src` plus the engine and externs it
links). Performance targets for this workload live in issue `waluau-yzt1`:

- Warm LSP clean edit: ≤ 250 ms median, ≤ 500 ms p95 (hard ceiling 1 s).
- Cold full build: ≥ 1 MB/s of resolved app source (~0.92 s for Ante).
- Peak RSS of a cold build: ≤ 1 GB (interim guardrail 2 GB, stretch 512 MB).

## `tools/benchmark-ante.mjs`

```bash
cargo build --release -p waluau-cli -p waluau-lsp
node tools/benchmark-ante.mjs
```

Scenarios, each reported with all samples, median, and p95:

- **cold** — full CLI builds in fresh processes. Timing covers the whole
  process, from input read through Wasm/JS artifact write; wasm-opt and
  compiler-binary construction are excluded by construction. Each sample also
  records peak RSS and peak memory footprint (`/usr/bin/time -l`, macOS) and
  the per-phase timings printed by `WALUAU_TIMINGS=1` (hir / symbols / ir /
  wasm / js).
- **lspCleanEditMs** — the primary editor-feedback number. One Ante root
  (`main.walu`) open in a warm `waluau-lsp` session; each sample is a valid
  whole-document change, timed from `didChange` until the server finished the
  triggered analysis. Completion is detected with a sentinel request queued
  behind the notification (the server is sequential and a clean analysis
  publishes no diagnostics), so the boundary is exactly
  "complete updated diagnostics are known".
- **lspErrorEditMs** — same edit shape but introducing a type error, reported
  separately because analysis short-circuits on the first error batch.
- **lspMultiRootCleanEditMs** — the clean edit with three Ante documents open
  (`main.walu`, `game.walu`, `flow.walu`). The LSP currently analyzes every
  open document as its own root per change, so this scenario tracks the
  multi-open-document cost explicitly.

Workload figures (`sourceUnits`, `appSourceBytes`, `linkedSourceBytes`,
post-link `astNodes`) come from the compiler's `--report` output
(`workload` key), so the benchmark fails loudly if the workload drifts from
what was measured. Cold throughput is `appSourceBytes / median wall time` —
raw resolved app source, matching the epic's definition.

Env knobs: `WALUAU_BENCH_COLD_SAMPLES` (default 5), `WALUAU_BENCH_LSP_SAMPLES`
(default 15), `WALUAU_BENCH_JSON=<path>` to also write the JSON result.

## `tools/benchmark-ante-incremental.mjs`

The older warm-rebuild benchmark: drives `waluau-cli --server` (the vite
plugin's compile server) through literal-toggle edits of one Ante file and
asserts the single-function incremental fast path stays under
`WALUAU_BENCH_THRESHOLD_MS`. Run it via `pnpm bench:ante-incremental`.

## Recording results

When a change lands that moves these numbers, paste the benchmark JSON (or
the relevant medians) into the issue that motivated the change, together with
the commit hash and the machine (the `meta` block carries commit, OS, and
CPU). Numbers quoted in `waluau-yzt1` were measured on the maintainer's
Apple-silicon macOS machine, release profile.
