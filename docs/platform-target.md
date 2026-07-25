# Platform target: Wasm GC, the DOM, and WebGPU

Status: accepted. Supersedes the "browser is the first platform" framing that
`engine/README.md` carried until `waluau-o0td`.

This is the one place where the project's platform is decided. Everything else
in the repository — module names, comments, issue descriptions, review feedback —
should be readable as a consequence of what is written here.

## Decision

Waluau compiles to **Wasm GC** and runs in **web browsers**. The **DOM** is the
host interface. **WebGPU** is the graphics and parallel-compute target.

There is one target, not a first target.

### Baseline

A supported host is a browser that provides, in a secure context:

- WebAssembly as the compiler emits it: GC types (`requiresWasmGc` reports when
  a module needs them) and exception handling (`try_table` plus an exported
  error tag, which is how `pcall` is lowered).
- The DOM surface generated into `externs/dom.walu`.
- `navigator.gpu`, with `requestAdapter()` returning an adapter that can create a
  device.

Current stable Chrome, Firefox, and Safari all meet this. The project does not
carry code for hosts that do not.

### Non-goals

These are closed questions. Reopening one takes a new decision in this file, not
a pull request that quietly adds a seam.

- **Native or server-side execution.** No Wasmtime/Wasmer/Node runtime target,
  no native CLI runtime for games. (The *compiler* is a native Rust binary; that
  is build tooling, not a runtime target.)
- **Native engine adapters.** No native resource, audio, filesystem, mixer, or
  save-data backend. Save slots are logical names because filesystem paths are
  the wrong shape for a browser, not because a filesystem host is coming.
- **WebGL and Canvas 2D as engine rendering paths.** They are not fallbacks and
  not a compatibility tier.
- **Renderer or backend portability.** No generic rendering interface whose
  purpose is to hide WebGPU, and no "this interface will survive other
  backends" promise in documentation or comments.

WebGL, Canvas 2D, and the rest of the DOM legitimately appear in `externs/` and
in `conformance/dom_*.walu`. Those files test the DOM extern bridge — typed-array
views over linear memory, spec-fixed enum constants, host-side pixel readback —
and the DOM is a target. The distinction is: DOM breadth in the bridge, WebGPU
only in the engine renderer.

### Transitional exception

`engine/graphics.walu`, the generated WebGL extern surface it uses, and the
`packages/vite-plugin-waluau` runtime that feeds it currently render through
WebGL2. This is debt from the period before this decision, tracked under
`waluau-o0td`. It is a migration in progress, not a supported backend: no new
feature should widen the WebGL surface, and nothing should be built to work
across both.

## Workload placement: what runs where

Wasm owns simulation state that gameplay reads back or branches on. WebGPU owns
work that is wide, uniform, and consumed by rendering.

Put work on the **GPU** when all three hold:

1. It is data-parallel over many elements (particles, sprites, tiles, physics
   broadphase cells).
2. Its results are consumed by rendering, or by another GPU pass, without a
   per-element CPU round trip.
3. Its state can live in GPU buffers between frames rather than being uploaded
   each frame.

Keep work in **Wasm** when it is control-flow heavy, when gameplay logic must
branch on individual results in the same frame, or when the element count is
small enough that a dispatch costs more than the loop.

Two consequences worth naming:

- **No per-element CPU tessellation or upload for large systems.** Building
  vertices on the CPU for tens of thousands of particles and streaming them
  through a general vertex batch each frame is the pattern this decision exists
  to prevent. Hot state stays in storage buffers; simulation, compaction, and
  draw happen as compute and instanced/indirect draw passes.
- **CPU-side snapshots are a debugging and conformance facility, not the
  architecture.** Where a subsystem needs a readable mirror of GPU state, that
  mirror is explicitly a test or inspection seam and is documented as one. It
  must not become the path a shipping frame takes.

Ergonomics do not have to lose. A game-facing module may stay deep and
LÖVE-shaped while its implementation is entirely WebGPU: bind groups, pipelines,
storage buffers, compute dispatch, and indirect draws are implementation detail
behind an interface that talks about particles, sprites, and colors.

## Seams

**One implementation means no seam.** Introduce an abstraction only when
behavior genuinely varies today, or when it buys concrete testing leverage
without enlarging the public platform contract.

Applied to what exists:

- `engine/browser.walu` **stays**, and it is not an "adapter" for a hypothetical
  platform. It exists so that game code never imports DOM externs: it owns
  canvas mounting, event registration, and `requestAnimationFrame`. That is a
  real separation of concerns with one implementation, described in terms of
  what it owns rather than what it abstracts over.
- Engine modules that avoid the DOM (`time.walu`, `input.walu`'s state,
  `particles.walu`'s simulation, a game's rules module) are described as
  **DOM-free** — they run in a headless test without a canvas. They are not
  described as host-independent or backend-neutral, because there is no second
  host to be independent of.
- The resource/audio/save import ABI stays Promise-shaped and handle-based
  because browser loading and decoding are asynchronous, not because another
  host might implement the same imports.

## Testing strategy

Three layers, with different determinism guarantees:

1. **DOM-free unit and simulation tests** (`fixtures/*/sim.walu`,
   `conformance/*.walu`) run without a canvas and are fully deterministic.
   Seeded randomness (`set_seed` on a particle system) belongs here. This is
   where simulation semantics are pinned.
2. **Browser conformance** (`apps/conformance-runner`) instantiates real
   modules in a real browser and verifies host-observable results, including
   pixel readback. GPU results are checked by reading back a buffer or texture
   and asserting on values or on tolerance-bounded pixels — never by comparing
   full-frame images across drivers.
3. **App suites** (`apps/playground`, `apps/ante`) cover application behavior.
   Compiler and engine contracts do not belong here.

GPU tests must assert on properties a conforming implementation guarantees:
buffer contents after a compute dispatch, a pixel's channel within a tolerance,
a draw count, a structured error code. Anything that depends on rasterization
rules a driver may vary is not a test.

### Headless WebGPU: verified constraint

Measured in this repository with Playwright 1.60 on macOS:

| Launch | `navigator.gpu` | `requestAdapter()` |
| --- | --- | --- |
| `about:blank` (any build) | absent | — |
| chromium **headless shell** (Playwright's default `headless: true`) | present | **null** |
| headless shell + `--enable-unsafe-swiftshader` | present | **null** |
| `channel: 'chromium'`, headless | present | adapter + device |

Two requirements follow, and both are load-bearing for any WebGPU test:

- **A secure context.** `navigator.gpu` is not exposed on `about:blank` or
  `data:` URLs. `http://localhost` counts as secure; the existing runners
  already serve over localhost.
- **Full Chromium, not the headless shell.** Suites that touch WebGPU must
  launch with `channel: 'chromium'`. On a CI runner without a GPU, add
  `--enable-unsafe-swiftshader` so the fallback adapter is available; it is
  harmless where a real adapter exists.

A WebGPU test that cannot get a device must fail loudly. Skipping on missing
`navigator.gpu` turns a broken runner into a green build.

## Consequences

- A new contributor can read the root `README.md` and this file and know what to
  build against without inferring it from the code.
- Review rejects: new WebGL or Canvas 2D engine rendering paths, fallback
  renderers, "for a future native host" justifications, abstractions with one
  implementation and no test leverage, and CPU-per-element paths for workloads
  that belong in a compute pass. `AGENTS.md` carries the short form of this for
  agents; the code-review skill reads it as a repo standard.
- Roadmap items that assumed portability are superseded rather than deferred:
  `waluau-foe0` (native resource/audio/save adapter) is closed as
  out-of-target.
- The WebGL2 renderer is a migration, and the migration is scheduled work rather
  than an aspiration in a document: `waluau-o0td.1` (test harnesses that can get
  a device) → `.2` (generated WebGPU externs plus a DOM-bridge conformance test)
  → `.3` (WebGPU host services in the Vite runtime) → `.4` (port
  `engine/graphics.walu`) → `.5` (game shaders to WGSL) and `.6` (GPU-resident
  particles) → `.7` (delete the WebGL2 path, which is what makes the
  transitional exception above expire).
