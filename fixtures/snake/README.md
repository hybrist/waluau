# Snake fixture

A small 2D snake game written in Waluau, rendered through the
`HTMLCanvasElement` / `CanvasRenderingContext2D` DOM externs added in #317.
It exercises canvas drawing, DOM construction, event callbacks, records,
growable arrays, modules, and a CSS-animation-driven game loop (see
`waluau-0nge` for why it is not `requestAnimationFrame`).

It is available in the playground as the "Snake Game" preset and covered by
`apps/playground/tests/dom-output.spec.js`.

## Layout

| File | Purpose |
| --- | --- |
| `main.walu` | Browser entry point: builds the DOM shell (canvas, d-pad buttons), wires click handlers, runs the tick loop. |
| `game.walu` | Host-independent game rules: state record, movement, food, collisions. |
| `render.walu` | Canvas 2D drawing (rectangles and text only, see TODOs). |
| `rng.walu` | Hand-rolled 31-bit LCG pseudo-random number generator. |
| `sim.walu` | Headless, DOM-free entry that asserts the game rules deterministically. |

## Building

```bash
# Browser entry (needs a DOM host such as the playground to actually run):
cargo run -p waluau-cli -- fixtures/snake/main.walu -o snake.wasm

# Headless rules check (runs entirely at wasm instantiation via asserts):
cargo run -p waluau-cli -- fixtures/snake/sim.walu -o sim.wasm
```

Both entries compile today. `sim.walu` traps on any rules regression, so it can
be executed in any wasm host that provides the standard `waluau` imports and
JS string-builtin constants. `main.walu` additionally needs the browser DOM
import surface used by the playground's DOM output mode.

## Known limitations exercised here

Each workaround in the source carries a `TODO(<beads-id>)` comment. Summary:

- `waluau-b3hl` — `fillStyle`/`strokeStyle` are Web IDL unions and are skipped
  by the extern generator, so all paint is black. Colors are faked by swapping
  the `ctx.filter` CSS-filter chain per draw call (`render.walu`), and the
  board backdrop is plain CSS `background-color` on the canvas element.
- `waluau-6kcc` — `fill()`/`stroke()` take an optional `Path2D` and need
  extern overloading, so built paths can never be painted. Everything is
  `fillRect`/`strokeRect`/`fillText`; the food is a square instead of a circle.
- `waluau-0nge` — host-invocable callback trampolines exist only for
  `(Event) -> unit` and `() -> extern`, so the `(f64) -> unit`
  `requestAnimationFrame` callback can never be called back (and there are no
  `setTimeout`/`setInterval` externs at all). The game loop is an invisible
  element with an infinite CSS animation whose `animationiteration` events
  tick the game through the supported `(Event) -> unit` trampoline.
- `waluau-6i00` — no `math.random`/`math.randomseed`; `rng.walu` hand-rolls an
  LCG seeded from the first tick's `Event.timeStamp`.
- `waluau-uzdp` — `KeyboardEvent` is not in the generated DOM surface, so
  there is no keyboard input; the d-pad is on-screen `<button>` elements.
- `waluau-ae6g` — module-local type aliases do not unify across module
  boundaries; the game-state record type is spelled out inline in every
  exported signature of `game.walu` and `render.walu`.
- `waluau-w5r0` — file-scope locals (including `require` bindings) are not
  visible from function bodies; dependencies are re-required inside each
  function that needs them.
