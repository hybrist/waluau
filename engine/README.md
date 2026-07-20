# Walua 2D engine

This directory defines the first usable slice of a 2D game engine written in
Waluau. It is intentionally smaller than LÖVE, but its boundaries are intended
to survive the addition of more complete backends and subsystems.

The browser is the first platform. Games depend on the engine facade while
`browser.walu` alone owns DOM setup, event registration, and
`requestAnimationFrame`. Drawing is GPU-backed: `graphics.walu` batches every
shape and text call into one interleaved vertex stream (clip-space position
plus color per vertex) and draws it through WebGL2 with a single draw call
per flush. Text uses either the built-in 5x7 bitmap font or a loaded custom
font rasterized once into a GPU glyph atlas; both render as colored quads.
The push/pop transform stack is applied CPU-side while vertices are emitted.
The fixed-step clock and keyboard state are host-independent.

## Initial API

```walu
local engine = require("waluau:engine")

local function update(dt: f64, input: engine.Input): unit
end

local function draw(graphics: engine.Graphics, alpha: f64): unit
end

local session: engine.Session = engine.start({
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

-- Stops the animation loop and unregisters browser input listeners.
session.stop()
```

## Package and version contract

The compiler embeds the engine sources. `waluau:engine` selects the current
stable major version and `waluau:engine/v1` pins major version 1. Both expose
the same aggregate facade:

- `VERSION`: the semantic API version (`1.2.0`)
- `start`: the browser lifecycle entry point
- `Config`, `Game`, `Session`, `Input`, and `Graphics`: canonical public types

Aggregate type exports are identity-preserving aliases. For example,
`engine.Input`, `input.Input`, and the `Input` accepted inside the browser
adapter all resolve to the same linked type declaration.

Subsystem modules remain supported for focused and host-independent programs:

| Current-major import | Pinned import | Surface |
| --- | --- | --- |
| `waluau:engine/input` | `waluau:engine/v1/input` | keyboard state and `Input` |
| `waluau:engine/graphics` | `waluau:engine/v1/graphics` | GPU drawing and `Graphics` |
| `waluau:engine/resources` | `waluau:engine/v1/resources` | packaged resource loading and handles |
| `waluau:engine/audio` | `waluau:engine/v1/audio` | decoded effects, streamed music, and playback control |
| `waluau:engine/time` | `waluau:engine/v1/time` | deterministic fixed-step clock |
| `waluau:engine/browser` | `waluau:engine/v1/browser` | browser lifecycle adapter |
| `waluau:engine/hot` | `waluau:engine/v1/hot` | development snapshot/restore registration |
| `waluau:engine/shader_sources` | `waluau:engine/v1/shader_sources` | revisioned external shader source polling |

Relative imports remain valid for engine development, but applications should
use package imports. See [`examples/game-project`](../examples/game-project/)
for the version-1 project layout and build/launch commands.

`Input` supplies `is_down`, `was_pressed`, and `was_released`. `Graphics`
supplies clearing, color and line-width state, rectangles, circles, lines,
text, and a push/pop transform stack with translate/rotate/scale. Every one
of those calls renders on the GPU: lines are thin quads, circles are
triangle fans, and `print` renders uppercased bitmap-font glyphs as quads,
so a frame batches into very few draw calls.

The WebGL backend also exposes `supports(name)`, game-provided `Shader`
resources, and `alpha`, `add`, and `multiply` blend modes. Shader creation
accepts vertex and pixel GLSL, returns structured compile/link diagnostics,
and has explicit lifetime; binding a released shader is rejected predictably.
The same backend uploads decoded image resources as textures, batches atlas
sprites by texture, and can render into and composite offscreen targets.
Loaded font resources become batched glyph atlases and keep the bitmap font as
a safe fallback.

Custom shaders consume any subset of the renderer's standard vertex attributes:
`a_position`, `a_color`, `a_uv`, and `a_textured`. The engine supplies
`u_texture`, live frame time in `u_time`, and logical-pixel scaling in
`u_pixel_scale` when those uniforms are declared. `use_shader` and
`use_default_shader` switch programs; float/vector and integer uniform setters
target the active program. `replace_shader` compiles and links a fresh program,
then atomically updates the existing `Shader` handle. A compile/link failure
returns structured data and leaves the prior program live; replacing an active
program flushes and rebinds it before deleting the old program. Program and
uniform changes flush pending geometry, so one batch never observes two shader
states.

For filled rectangles, `a_uv` spans `(0, 0)` at the top-left to `(1, 1)` at
the bottom-right even though `a_textured` is zero. This gives procedural
shaders stable shape-local coordinates that follow CPU-side transforms.
Sprites use their atlas UVs with `a_textured` set to one; other untextured
primitives currently receive zero UVs.

```walu
local result = graphics:create_shader(vertex_source, pixel_source)
if result.ok then
    graphics:use_shader(result.shader)
    graphics:set_uniform_float("u_strength", 0.8)
    graphics:fill_rectangle(24.0, 24.0, 96.0, 48.0)
    graphics:use_default_shader()
    graphics:release_shader(result.shader)
else
    print(result.error.code .. ": " .. result.error.message)
end
```

`keyreleased` may be `nil`; the other lifecycle callbacks are currently
required. Updates use a fixed timestep. Drawing happens once per animation
frame and receives an interpolation alpha in `[0, 1)`. Long frame delays are
clamped by `max_frame_seconds` to avoid an unbounded catch-up loop.
`start` returns a `Session`; `Session.stop()` is idempotent and releases the
browser lifecycle registrations and mounted root owned by that run. Development
hot replacement uses `Session.suspend()` through a game's
`waluau:engine/hot` dispose closure; it releases the callbacks while keeping
the last frame mounted until the replacement presents its first frame.

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
    local result: resources.LoadResult = resources.await_text(path)
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

The `await_*` helpers suspend the owning coroutine and return structured results.
The lower-level request/finish pairs remain available: `load_*` returns the host
promise, and `finish_*` copies its resolved integer handle into Waluau records.
The service ABI remains Promise-based; host declarations use bare `extern`
promises until module-local generic Promise aliases link across required modules.

Packaged assets and save data are deliberately separate:

- `resources.await_text/await_bytes/await_image/await_font` load validated,
  read-only project paths and produce a `LoadResult`; matching `load_*` and
  `finish_*` calls expose the two-phase form. Browser loads use `fetch`; image
  and font handles do not become ready until decoding succeeds.
- `audio.load_sound` fully downloads and decodes an effect. `load_stream`
  readies a streaming media source. Call `audio.unlock()` from a user-input
  callback to satisfy browser autoplay policy, and hold time-driven effects
  until that unlock attempt succeeds. Decoded effects are never queued while
  the browser audio context is suspended. `play` returns `false` when playback
  cannot start, so games can surface or otherwise handle a playback failure
  explicitly.
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
`font_from_resource` similarly copies a loaded font into a GPU atlas;
`set_font_resource` selects it at a logical-pixel size, while `font_error`
exposes invalid or `released` status and `use_builtin_font` restores the
always-available fallback. Releasing the decoded FontFace after atlas creation
does not invalidate already-uploaded glyphs.

The version-1 `waluau.assets.json` contract and `--manifest` CLI option package
typed, read-only project assets with content fingerprints. Generated sibling
glue exports the logical-path manifest and an import-meta-relative base URL;
the browser host maps logical requests to emitted URLs and rejects undeclared
or wrongly typed requests. Save namespaces never consult this manifest. See
[`examples/game-project`](../examples/game-project/) for the complete layout
and build command. A native resource adapter remains a separate follow-up.

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
| GPU graphics | Typed buffers, numeric/vector data, shader and uniform-friendly APIs | WebGL2 geometry, texture/sprite/glyph batching, render targets, game-provided vertex/pixel shader compilation, structured diagnostics, uniforms, and explicit shader lifetime are available |
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

The renderer draws colored geometry, loaded textures, bitmap or custom-font
glyphs, atlas sprite batches, and render targets through WebGL2 today, using
the extern surface from `waluau-9tvw`. Games can compile and bind their own
vertex/pixel programs on that same batched stream.
