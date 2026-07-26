# Platform target: Wasm GC in the browser

Status: accepted. Supersedes the "browser is the first platform" framing that
`engine/README.md` carried until `waluau-o0td`.

This is the one place where the project's platform is decided. Everything else
in the repository — module names, comments, issue descriptions, review feedback —
should be readable as a consequence of what is written here.

## Decision

Waluau compiles to **Wasm GC** and runs in **web browsers**. The **DOM** is the
host interface. The engine draws on the GPU through **WebGL2**, which is a DOM
API like any other.

There is one target, not a first target. The repository documents what it
supports, not what it might support later.

### Baseline

A supported host is a browser that provides:

- WebAssembly as the compiler emits it: GC types (`requiresWasmGc` reports when
  a module needs them) and exception handling (`try_table` plus an exported
  error tag, which is how `pcall` is lowered).
- The DOM surface generated into `externs/dom.walu`.
- A WebGL2 context.

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
- **Renderer or backend portability.** No generic rendering interface whose
  purpose is to hide WebGL2, and no "this interface will survive other backends"
  promise in documentation or comments. The renderer is free to be WebGL2 all
  the way down.
- **Speculative migration.** Replacing the renderer with a different graphics
  API is not planned work, and nothing should be built, staged, or worded to
  prepare for it. If a concrete need ever justifies it, that is a decision
  recorded here first — not groundwork laid in advance.

WebGL, Canvas 2D, and the rest of the DOM appear in `externs/` and in
`conformance/dom_*.walu`. That is in-target by construction: those files test the
DOM extern bridge — typed-array views over linear memory, spec-fixed enum
constants, host-side pixel readback — and the DOM is the host interface.

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
- `engine/graphics.walu` talks WebGL2 directly, and should keep doing so. Its
  job is to give games a `Graphics` API that describes shapes, sprites, and
  colors rather than buffers and programs — that keeps game code out of the
  host, which is a different goal from hiding the renderer. Where showing WebGL2
  through the interface is the faster or simpler design, show it.

The resource/audio/save import ABI stays Promise-shaped and handle-based
because browser loading and decoding are asynchronous, not because another host
might implement the same imports.

## Testing strategy

Three layers, with different determinism guarantees:

1. **DOM-free unit and simulation tests** (`fixtures/*/sim.walu`,
   `conformance/*.walu`) run without a canvas and are fully deterministic.
   Seeded randomness (`set_seed` on a particle system) belongs here. This is
   where simulation semantics are pinned.
2. **Browser conformance** (`apps/conformance-runner`) instantiates real
   modules in a real browser and verifies host-observable results, including
   pixel readback through `preserveDrawingBuffer` and `readPixels`.
3. **App suites** (`apps/playground`, `apps/ante`) cover application behavior.
   Compiler and engine contracts do not belong here.

GPU tests must assert on properties a conforming implementation guarantees: a
pixel's channel within a tolerance, a draw count, a structured error code.
Anything that depends on rasterization rules a driver may vary is not a test.

## Consequences

- A new contributor can read the root `README.md` and this file and know what to
  build against without inferring it from the code.
- Review rejects: "for a future native host" justifications, abstractions with
  one implementation and no test leverage, a rendering interface added to hide
  WebGL2, and work whose only purpose is to prepare for a platform or API this
  project has not decided to adopt. `AGENTS.md` carries the short form of this
  for agents; the code-review skill reads it as a repo standard.
- Roadmap items that assumed portability are superseded rather than deferred:
  `waluau-foe0` (native resource/audio/save adapter) is closed as
  out-of-target.
