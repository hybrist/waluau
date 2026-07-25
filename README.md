# Waluau

Waluau is a statically typed Luau dialect that compiles to WebAssembly and runs
in web browsers. The repository holds the compiler, the browser toolchain around
it, and a 2D game engine written in Waluau itself.

## Target

**Waluau targets Wasm GC in web browsers. Nothing else.**

- **Wasm GC** is the execution target. Game logic, engine logic, and runtime
  data structures live in the Wasm module.
- **The DOM** is the host interface. Browser APIs reach Waluau through generated
  externs (`externs/`), not through a hand-written portability layer.
- **WebGPU** is the graphics and parallel-compute target. Rendering, and
  workloads such as large particle simulations where GPU execution gives
  material leverage, belong on the GPU.

Explicit non-goals: native or server-side execution, native engine adapters
(resources, audio, filesystem, mixer), WebGL and Canvas 2D as engine rendering
paths, and renderer or backend portability of any kind. Do not add an
abstraction whose only justification is a platform this project does not ship.

`engine/graphics.walu` still renders through WebGL2. That is transitional debt,
tracked under `waluau-o0td`, not a supported second backend.

WebGL and Canvas 2D *do* appear in `externs/` and in `conformance/dom_*.walu`.
That is deliberate: those files test the DOM extern bridge itself (typed-array
views, spec enum constants, host-side readback), and the DOM is a target.

See [`docs/platform-target.md`](docs/platform-target.md) for the full decision:
supported baseline, CPU-versus-GPU placement, compatibility policy, and how
GPU-resident behavior is tested deterministically.

## Layout

| Path | Contents |
| --- | --- |
| `crates/` | The Rust compiler: lexer, parser, HIR/type checker, IR, Wasm codegen, formatter, LSP, CLI |
| `crates/waluau-wasm/` | The compiler itself compiled to Wasm, for in-browser builds (playground, tests) |
| `engine/` | The 2D game engine, written in Waluau |
| `externs/` | Generated DOM extern declarations (`dom.walu`) |
| `tools/dom-idl/` | Generator that turns Web IDL into `externs/dom.walu` |
| `packages/vite-plugin-waluau/` | Vite plugin and browser runtime that build and instantiate `.walu` entry points |
| `apps/playground/` | Browser playground and its Playwright suite |
| `apps/ante/` | Arcane Heist, the reference game |
| `apps/conformance-runner/` | Headless-browser runner for the conformance suite |
| `conformance/` | Language and DOM-bridge conformance tests, written in Waluau |
| `fixtures/` | Larger engine programs used as tests and playground presets |
| `examples/game-project/` | The version-1 standalone project layout |

## Getting started

Compile a program to Wasm plus JavaScript glue:

```bash
cargo run -- fixtures/snake/main.walu -o /tmp/snake.wasm --emit-js
```

Run the Rust checks (format, build, tests, clippy):

```bash
./check
```

Run the browser suites:

```bash
pnpm install && pnpm test:conformance-browser && pnpm test:playground && pnpm test:ante
```

Start the playground:

```bash
pnpm dev
```

## Where the durable documentation lives

- Platform target and architecture decision: [`docs/platform-target.md`](docs/platform-target.md)
- Engine API and contracts: [`engine/README.md`](engine/README.md)
- Conformance test format and directives: [`conformance/README.md`](conformance/README.md)
- Agent and contributor workflow: [`AGENTS.md`](AGENTS.md)
- Planned work, gaps, and priorities: beads (`bd ready`, `bd show <id>`)

Language and compiler behavior is documented by the tests: `crates/*/src/tests.rs`,
`conformance/`, and `fixtures/`. `docs/` stays deliberately small — see
[`docs/README.md`](docs/README.md).
