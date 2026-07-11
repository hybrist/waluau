# Snake fixture

A small 2D snake game written in Waluau and hosted by the engine under
`engine/`. The fixture itself has no DOM, canvas-context, event-listener, or
animation-frame code. It exercises the engine lifecycle, fixed-step updates,
keyboard callbacks, GPU-backed shape/text drawing, records, growable arrays,
modules, and the `math.random`/`math.randomseed` host builtins.

It is available in the playground as the "Snake Game" preset and covered by
`apps/playground/tests/dom-output.spec.js`.

## Layout

| File | Purpose |
| --- | --- |
| `main.walu` | Engine entry point: configures the board, owns callbacks, and maps keyboard presses to game actions. |
| `game.walu` | Host-independent game rules: state record, movement, food (placed via `math.random`), collisions. |
| `render.walu` | Platform-independent `Graphics` drawing: arena/snake rectangles, circular food, and bitmap-font HUD. |
| `sim.walu` | Headless, DOM-free entry that asserts the game rules deterministically (seeded via `math.randomseed`). |

## Building

```bash
# Browser entry (the engine's browser adapter needs a DOM/WebGL2 host):
cargo run -p waluau-cli -- fixtures/snake/main.walu -o snake.wasm

# Headless rules check (runs entirely at wasm instantiation via asserts):
cargo run -p waluau-cli -- fixtures/snake/sim.walu -o sim.wasm
```

Both entries compile today. `sim.walu` traps on any rules regression, so it can
be executed in any wasm host that provides the standard `waluau` imports and
JS string-builtin constants. `main.walu` additionally needs the DOM and WebGL2
surface used internally by the engine's browser and graphics adapters.

The port currently uses a deterministic seed because the engine does not yet
expose platform entropy (`waluau-qr53`). The previous touch-friendly on-screen
controls remain deferred until the engine has pointer/touch input
(`waluau-chsb`); keyboard arrows/WASD and `R` are supported now.
