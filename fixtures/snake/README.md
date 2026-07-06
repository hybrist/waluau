# Snake fixture

A small 2D snake game written in Waluau, rendered through the
`HTMLCanvasElement` / `CanvasRenderingContext2D` DOM externs added in #317.
It exercises canvas drawing, DOM construction, event callbacks, records,
growable arrays, modules, the `math.random`/`math.randomseed` host builtins,
and a `requestAnimationFrame`-driven game loop (invoked back into wasm
through the `(f64) -> unit` callback trampoline).

It is available in the playground as the "Snake Game" preset and covered by
`apps/playground/tests/dom-output.spec.js`.

## Layout

| File | Purpose |
| --- | --- |
| `main.walu` | Browser entry point: builds the DOM shell (canvas, d-pad buttons), wires keyboard + click handlers, runs the tick loop. |
| `game.walu` | Host-independent game rules: state record, movement, food (placed via `math.random`), collisions. |
| `render.walu` | Canvas 2D drawing: arena and snake rectangles, circular food via `beginPath()`/`arc()`/`fill()`, text HUD. |
| `sim.walu` | Headless, DOM-free entry that asserts the game rules deterministically (seeded via `math.randomseed`). |

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
