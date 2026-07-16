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
local engine = require("waluau:engine")

local function update(dt: f64, input: engine.Input): unit
end

local function draw(graphics: engine.Graphics, alpha: f64): unit
end

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

## Package and version contract

The compiler embeds the engine sources. `waluau:engine` selects the current
stable major version and `waluau:engine/v1` pins major version 1. Both expose
the same aggregate facade:

- `VERSION`: the semantic API version (`1.0.0`)
- `start`: the browser lifecycle entry point
- `Config`, `Game`, `Input`, and `Graphics`: canonical public types

Aggregate type exports are identity-preserving aliases. For example,
`engine.Input`, `input.Input`, and the `Input` accepted inside the browser
adapter all resolve to the same linked type declaration.

Subsystem modules remain supported for focused and host-independent programs:

| Current-major import | Pinned import | Surface |
| --- | --- | --- |
| `waluau:engine/input` | `waluau:engine/v1/input` | keyboard state and `Input` |
| `waluau:engine/graphics` | `waluau:engine/v1/graphics` | GPU drawing and `Graphics` |
| `waluau:engine/resources` | `waluau:engine/v1/resources` | packaged resource loading and handles |
| `waluau:engine/time` | `waluau:engine/v1/time` | deterministic fixed-step clock |
| `waluau:engine/browser` | `waluau:engine/v1/browser` | browser lifecycle adapter |

Relative imports remain valid for engine development, but applications should
use package imports. See [`examples/game-project`](../examples/game-project/)
for the version-1 project layout and build/launch commands.

`Input` supplies `is_down`, `was_pressed`, and `was_released`. `Graphics`
supplies clearing, color and line-width state, rectangles, circles, lines,
text, and a push/pop transform stack with translate/rotate/scale. Every one
of those calls renders on the GPU: lines are thin quads, circles are
triangle fans, and `print` renders uppercased bitmap-font glyphs as quads,
so a frame batches into very few draw calls.

The WebGL backend also exposes `supports(name)`, portable named `Material`
resources, and `alpha`, `add`, and `multiply` blend modes. Material creation
returns a structured result; releasing a material is explicit and later use is
rejected predictably. The same backend uploads decoded image resources as
textures, batches atlas sprites by texture, and can render into and composite
offscreen targets. Custom shaders remain unsupported and are tracked by
`waluau-ukso`; compatibility backends must likewise return `false` instead of
silently changing semantics.

`keyreleased` may be `nil`; the other lifecycle callbacks are currently
required. Updates use a fixed timestep. Drawing happens once per animation
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
smallest browser example. [`fixtures/snake/`](../fixtures/snake/) is a fuller
port with separate rules and rendering modules; none of its game code mentions
DOM types, canvas contexts, event listeners, or animation frames.
[`fixtures/game-engine/sim.walu`](../fixtures/game-engine/sim.walu) runs the
engine clock and input state without a browser.

## Resource, audio, and save-data services

Games import [`resources.walu`](resources.walu), [`audio.walu`](audio.walu),
and [`save.walu`](save.walu); none of those modules expose DOM objects. Loads
run inside an ordinary Waluau coroutine and return a structured result:

```walu
local resources = require("../../engine/resources")

local co = coroutine.create(function(): i32
    local path: string = "assets/level.txt"
    local handle: i32 = coroutine.await_promise(resources.load_text(path))::i32
    local result: resources.LoadResult = resources.finish_text(handle, path)
    if result.ok then
        print(resources.text(result.resource))
        resources.release(result.resource)
    else
        print(result.error.code .. ": " .. result.error.message)
    end
    return 0
end)
coroutine.resume(co)
```

Each load has an explicit request/finish pair. `load_*` returns the host
promise; the owning coroutine awaits its integer handle, then `finish_*` copies
readiness or failure into Waluau records. Keeping the suspension in the caller
is a temporary language/runtime constraint: module-local generic Promise aliases
in host declarations and transitive awaits that preserve aggregate caller
locals are tracked as compiler follow-ups. The service ABI itself remains
Promise-based and does not change when those wrappers become expressible.

Packaged assets and save data are deliberately separate:

- `resources.load_text/load_bytes/load_image/load_font` request validated,
  read-only project paths; their matching `finish_*` functions produce a
  `LoadResult`. Browser loads use `fetch`; image and font handles do not become
  ready until decoding succeeds.
- `audio.load_sound` fully downloads and decodes an effect. `load_stream`
  readies a streaming media source. `play` returns `false` when playback cannot
  start, so browsers may retry from a user-input callback.
- `save.read_*/write_*/delete` use logical slot names in a versioned,
  per-game namespace. They are asynchronous even though the browser adapter
  uses local storage, preserving the API for future native atomic file I/O.

Host promises always resolve to either a ready handle or a structured failure;
raw fetch, decode, storage, and device exceptions do not cross into game code.
Stable error codes currently include `invalid_path`, `invalid_key`,
`not_found`, `http_error`, `decode_failed`, `unavailable`,
`permission_denied`, `storage_full`, `storage_failed`, and `wrong_type`.
Manifest-backed hosts additionally report `undeclared_asset`,
`wrong_asset_type`, and `invalid_manifest` without attempting a network load.

Every successful load owns one handle. Call `resources.release` when it is no
longer needed; release is idempotent and closes image objects, unregisters font
faces, stops decoded sources, and detaches streams. This first slice has no
implicit cache. [`fixtures/game-engine/resources.walu`](../fixtures/game-engine/resources.walu)
is the backend-neutral contract sample for image/font/audio readiness, safe
failure, and save reload behavior.

`Graphics:texture_from_resource` copies a ready image into GPU storage without
fetching or decoding it again. The decoded source may therefore be released as
soon as upload succeeds. `draw_image` and `draw_sprite` use top-left logical
coordinates (including atlas source rectangles); consecutive sprites that use
the same texture remain in one batch. `create_render_target`,
`set_render_target`, `set_screen_target`, and `draw_render_target` provide
offscreen composition. Texture and target release is explicit and idempotent,
and later use returns `false`; creation returns structured `invalid_resource`,
`wrong_type`, `invalid_size`, `unavailable`, upload, and framebuffer failures.

The version-1 `waluau.assets.json` contract and `--manifest` CLI option package
typed, read-only project assets with content fingerprints. Generated sibling
glue exports the logical-path manifest and an import-meta-relative base URL;
the browser host maps logical requests to emitted URLs and rejects undeclared
or wrongly typed requests. Save namespaces never consult this manifest. See
[`examples/game-project`](../examples/game-project/) for the complete layout
and build command. A native adapter and GPU glyph integration for loaded font
handles remain separate follow-ups.

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
| Distribution | Stable package/virtual-module imports, API versioning and a standard project layout are available | Generated sibling JavaScript glue and typed fingerprinted asset manifests are available |
| Resources | Backend-neutral opaque handles, nullable callbacks and host byte transfer are available | Browser async text/bytes/image/font/audio handles, decoded-image GPU upload, explicit lifetime, structured failures, and production packaging are available; caching and native adapters remain |
| GPU graphics | Typed buffers, numeric/vector data, shader and uniform-friendly APIs | WebGL2 geometry, texture/sprite batching and render targets are available; custom shader compilation, WebGPU/native adapters and broader capability discovery remain |
| Input | Extensible event/value representation without a closed hard-coded record | Keyboard normalization, pointer/touch/gamepad polling, focus and fullscreen handling |
| Audio | Optional readiness callbacks and richer source state | Browser decoded effects and streamed music; native mixer, buses, effects and device lifecycle remain |
| Files | Stable project/package layout | Browser packaged fetch plus namespaced text/byte saves; desktop paths and atomic native save adapter remain |
| Tooling | Source locations and protected error propagation across host callbacks | Project runner, asset pipeline, hot reload, debugger/profiler and distributable packaging |
| Performance | Predictable allocation, reusable buffers, broader numeric/vector operations | Batched submission, off-main-thread work where available, frame/memory profiling |

Several prerequisites already have repository issues, including 3D/GPU canvas
access (`waluau-9tvw`), generated JavaScript/Wasm glue (`waluau-884g`), dynamic
extern values (`waluau-lxdd`), host container marshalling (`waluau-utyc`), and
host-boundary error catching (`waluau-uvfk`). Engine-specific follow-ups cover
the stable package surface (`waluau-tpil`), GPU-backed renderer (`waluau-vt3k`),
and asset/audio/save services (`waluau-mi1t`). Beads remains the authoritative
source for priorities and completion status.

The renderer draws colored geometry (shapes, lines, paths, and bitmap-font
text) through WebGL2 today, using the extern surface from `waluau-9tvw`.
Loaded textures, atlas sprite batches, and render targets are implemented by
the browser WebGL2 backend. Custom shader resources remain an explicit follow-up
of `waluau-vt3k`; a Canvas 2D compatibility backend can return once backend
polymorphism is expressible in the language.
