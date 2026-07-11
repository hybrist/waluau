# Walua 2D engine

This directory defines the first usable slice of a 2D game engine written in
Waluau. It is intentionally smaller than LÖVE, but its boundaries are intended
to survive the addition of more complete backends and subsystems.

The browser is the first platform. Games depend on the engine facade while
`browser.walu` alone owns DOM setup, event registration, and
`requestAnimationFrame`. Drawing is GPU-backed: `graphics.walu` batches every
shape and text call into one interleaved vertex stream (clip-space position
plus color per vertex) and draws it through WebGL2 with a single draw call
per flush. Text uses the built-in 5x7 bitmap font in `font.walu`, rendered as
colored quads, and the push/pop transform stack is applied CPU-side while
vertices are emitted. The fixed-step clock and keyboard state are
host-independent.

## Initial API

```walu
local engine = require("../../engine/browser")

engine.start({
    title = "My game",
    width = 640,
    height = 360,
    update_hz = 60.0,
    max_frame_seconds = 0.25,
    background = "#000000",
}, {
    load = load,
    update = update,             -- (fixed_dt_seconds, Input) -> unit
    draw = draw,                 -- (Graphics, interpolation_alpha) -> unit
    keypressed = keypressed,
    keyreleased = keyreleased,
})
```

`Input` supplies `is_down`, `was_pressed`, and `was_released`. `Graphics`
supplies clearing, color and line-width state, rectangles, circles, lines,
text, and a push/pop transform stack with translate/rotate/scale. Every one
of those calls renders on the GPU: lines are thin quads, circles are
triangle fans, and `print` renders uppercased bitmap-font glyphs as quads,
so a frame batches into very few draw calls.

Callbacks are mandatory for now; games use a no-op callback when they do not
need a hook. Updates use a fixed timestep. Drawing happens once per animation
frame and receives an interpolation alpha in `[0, 1)`. Long frame delays are
clamped by `max_frame_seconds` to avoid an unbounded catch-up loop.

## Porting a simple game

A small game needs to do four things:

1. Move persistent gameplay data into typed Waluau records and arrays.
2. Move initialization, fixed-step simulation, and rendering into `load`,
   `update`, and `draw` callbacks.
3. Replace direct platform input with `Input` queries or key callbacks.
4. Replace direct canvas calls with `Graphics` methods, keeping coordinates in
   logical canvas pixels.

[`fixtures/game-engine/main.walu`](../fixtures/game-engine/main.walu) is the
smallest browser example. Unlike the older Snake fixture, its game module does
not mention DOM types, canvas contexts, or animation frames.
[`fixtures/game-engine/sim.walu`](../fixtures/game-engine/sim.walu) runs the
engine clock and input state without a browser.

## Intended API surface

The long-term public surface should remain small and subsystem-oriented:

- `engine`: lifecycle, configuration, quit/restart, errors, version/features
- `time`: fixed/variable timing, timer, sleep where a platform permits it
- `input`: keyboard, pointer, touch, gamepad, text entry, focus
- `graphics`: windows/canvases, colors, transforms, shapes, text, images,
  atlases, sprite batches, meshes, render targets, blend/depth/stencil state,
  shaders and capability queries
- `audio`: decoded/streamed sources, playback state, buses and spatial audio
- `filesystem`: packaged assets, save data and platform-safe paths
- `physics`: optional 2D collision and rigid-body integration
- `math`: vectors, matrices, rectangles, interpolation, noise and randomness
- `system`: OS/display information, clipboard, URLs and diagnostics

The API should describe resources and commands rather than expose browser DOM
objects. That permits Canvas 2D, WebGL/WebGPU, and future native renderers to
share game-facing concepts even where their performance characteristics differ.

## Capabilities required for a complete engine

The current language/runtime is sufficient for small shape-based browser games.
Reaching LÖVE-like usability requires these architectural capabilities:

| Area | Waluau/language requirement | Platform/runtime requirement |
| --- | --- | --- |
| Distribution | Stable package/virtual-module imports instead of repository-relative paths; API versioning | Generated loader/glue and a standard game project/build layout |
| Resources | General resource handles, nullable callbacks, ergonomic byte buffers and host-array transfer | Async image/font/audio decoding, caching, lifetime management and failure reporting |
| GPU graphics | Typed buffers, numeric/vector data, shader and uniform-friendly APIs | WebGL/WebGPU or native GPU backend, batching, render targets, shader compilation and capability discovery |
| Input | Extensible event/value representation without a closed hard-coded record | Keyboard normalization, pointer/touch/gamepad polling, focus and fullscreen handling |
| Audio | Stable opaque handles and callbacks/promises for asynchronous readiness | Web Audio/native mixer, streaming, buses, effects and device lifecycle |
| Files | Byte/string I/O results and structured errors | Packaged assets, browser fetch/storage, desktop paths and save-data policy |
| Tooling | Source locations and protected error propagation across host callbacks | Project runner, asset pipeline, hot reload, debugger/profiler and distributable packaging |
| Performance | Predictable allocation, reusable buffers, broader numeric/vector operations | Batched submission, off-main-thread work where available, frame/memory profiling |

Several prerequisites already have repository issues, including 3D/GPU canvas
access (`waluau-9tvw`), generated JavaScript/Wasm glue (`waluau-884g`), dynamic
extern values (`waluau-lxdd`), host container marshalling (`waluau-utyc`), and
host-boundary error catching (`waluau-uvfk`). Engine-specific follow-ups cover
the stable package surface (`waluau-tpil`), GPU-backed renderer (`waluau-vt3k`),
and asset/audio/save services (`waluau-mi1t`). Beads remains the authoritative
source for priorities and completion status.

The renderer draws colored geometry (shapes, lines, and bitmap-font text)
through WebGL2 today, using the extern surface from `waluau-9tvw`. Textures,
sprite batches, render targets, and custom shaders remain the domain of the
full GPU backend tracked by `waluau-vt3k`; a Canvas 2D compatibility backend
can return once backend polymorphism is expressible in the language.
