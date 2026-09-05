# Walua 2D engine

This directory defines the first usable slice of a 2D game engine written in
Waluau. It is intentionally smaller than LÖVE; the subsystem boundaries are
meant to hold as more of LÖVE's surface arrives behind them.

The engine runs in a browser: Wasm GC for logic, the DOM for hosting, WebGL2 for
drawing. See [`docs/platform-target.md`](../docs/platform-target.md) for the
target and its non-goals.

Games depend on the engine facade while `browser.walu` alone owns DOM setup,
event registration, and `requestAnimationFrame`. Drawing is GPU-backed:
`graphics.walu` batches every shape and text call into one interleaved vertex
stream (clip-space position plus color per vertex) and draws it through WebGL2
with a single draw call per flush. Text uses either the built-in 5x7 bitmap font
or a loaded custom font rasterized once into a GPU glyph atlas; both render as
colored quads. The push/pop transform stack is applied CPU-side while vertices
are emitted. The fixed-step clock and keyboard state are DOM-free, so they run
in a headless test without a canvas.

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
    mousepressed = mousepressed,   -- (x, y, button) -> unit
    mousereleased = mousereleased, -- (x, y, button) -> unit
    mousemoved = mousemoved,       -- (x, y, dx, dy) -> unit
})

-- Stops the animation loop and unregisters browser input listeners.
session.stop()

-- Changes the logical viewport without starting over, so everything already
-- loaded into the renderer stays valid.
session.resize(800, 480)
```

`Config.mount` names an element to put the game inside. Left out, the document
body hosts it and the game owns the page; named, the engine styles only what it
mounted, which is what lets a host page — Storybook's preview — keep its own
layout around the canvas.

`Config.canvas_sizing = true` makes the canvas fill the browser's available
space and updates `Graphics.width` and `Graphics.height` to its live CSS size.
Without it, `width` and `height` remain the fixed logical viewport.

`Config.width` and `Config.height` are optional. Left out, the engine measures
the browser's available space instead — the natural pairing with
`canvas_sizing`, where the canvas has no one true size to be given and the
game derives its layout from the live dimensions each frame.

## Pointing devices

The canvas listens for DOM pointer events, so mouse, pen and touch contacts all
arrive through `mousepressed`, `mousereleased` and `mousemoved` — in logical
canvas coordinates, with Love2D-style button numbers where a finger or a pen tip
presses button 1. `Input.mouse_x`, `Input.mouse_y` and `is_mouse_down` report the
same stream to a polling game. A game therefore gets touch input by handling the
callbacks it already handles; there is no separate touch path to write.

The canvas takes `touch-action: none`, so the browser never reinterprets a
gesture that lands on it as a scroll, a pinch zoom, or the first half of a
double-tap zoom, and never holds a tap back waiting to find out. A cancelled
gesture — one the browser takes away rather than the player lifting — puts the
buttons back up without reporting a release, because no release happened.

A game that fills the window should also stop the page itself from zooming
around it, which is a document concern rather than an engine one:

```html
<meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no" />
```

The engine sizes the canvas from the space the browser actually has, and its
root box is `dvh` rather than `vh` so that a mobile browser showing its own bars
does not lay out a root taller than the screen and push the canvas half off it.
Those bars are the browser's, though: they collapse on a page scroll, and a game
that fills the viewport has nothing to scroll. Losing them is a matter of how
the page is opened rather than anything a page can ask for, so a game meant to
be played on a phone or a tablet should also say it can be installed:

```html
<meta name="mobile-web-app-capable" content="yes" />
<meta name="apple-mobile-web-app-capable" content="yes" />
```

## Package and version contract

The compiler embeds the engine sources. `waluau:engine` selects the current
stable major version and `waluau:engine/v1` pins major version 1. Both expose
the same aggregate facade:

- `VERSION`: the semantic API version (`1.7.0`)
- `start`: the browser lifecycle entry point
- `prefers_reduced_motion`: whether the browser's `prefers-reduced-motion` media
  query matches, for decoration that would otherwise move on its own
- `Config`, `Game`, `Session`, `Input`, `Graphics`, and `ParticleSystem`:
  canonical public types
- `new_particle_system`, `particle_color`, `particle_hex`, and `particle_quad`:
  particle constructors

Aggregate type exports are identity-preserving aliases. For example,
`engine.Input`, `input.Input`, and the `Input` accepted inside `browser.walu`
all resolve to the same linked type declaration.

Subsystem modules remain supported for focused programs, including DOM-free ones:

| Current-major import | Pinned import | Surface |
| --- | --- | --- |
| `waluau:engine/input` | `waluau:engine/v1/input` | keyboard and pointer state and `Input` |
| `waluau:engine/graphics` | `waluau:engine/v1/graphics` | GPU drawing and `Graphics` |
| `waluau:engine/particles` | `waluau:engine/v1/particles` | pooled particle emitters and `ParticleSystem` |
| `waluau:engine/resources` | `waluau:engine/v1/resources` | packaged resource loading and handles |
| `waluau:engine/audio` | `waluau:engine/v1/audio` | decoded effects, streamed music, and playback control |
| `waluau:engine/time` | `waluau:engine/v1/time` | deterministic fixed-step clock |
| `waluau:engine/browser` | `waluau:engine/v1/browser` | DOM setup, input events, and the animation loop |
| `waluau:engine/hot` | `waluau:engine/v1/hot` | development snapshot/restore registration |
| `waluau:engine/shader_sources` | `waluau:engine/v1/shader_sources` | revisioned external shader source polling |
| `waluau:engine/storybook` | `waluau:engine/v1/storybook` | story declarations for Storybook (`@waluau/storybook`) |

Relative imports remain valid for engine development, but applications should
use package imports. See [`examples/game-project`](../examples/game-project/)
for the version-1 project layout and build/launch commands.

`Input` supplies `is_down`, `was_pressed`, and `was_released` for keys, and
`mouse_x`, `mouse_y`, `is_mouse_down`, `was_mouse_pressed`, and
`was_mouse_released` for whichever pointing device is in the player's hand.
`Graphics`
supplies clearing, color and line-width state, rectangles, circles, lines,
text, and a push/pop transform stack with translate/rotate/scale. Every one
of those calls renders on the GPU: lines are thin quads, circles are
triangle fans, and `print` renders uppercased bitmap-font glyphs as quads,
so a frame batches into very few draw calls.

The WebGL2 renderer also exposes `supports(name)`, game-provided `Shader`
resources, and `alpha`, `add`, and `multiply` blend modes. Shader creation
accepts vertex and pixel GLSL, returns structured compile/link diagnostics, and
has explicit lifetime; binding a released shader is rejected predictably. The
same renderer uploads decoded image resources as textures, batches atlas sprites
by texture, and can render into and composite offscreen targets. Loaded font
resources become batched glyph atlases and keep the bitmap font as a safe
fallback.

Colors can be set numerically with `set_color_rgba` when a color is computed
rather than written down, `fill_quad` fills an arbitrary quadrilateral (which
is how rotated particles and beams reach the batch without a transform push),
and `texture_from_render_target` views a target's color storage as a sampleable
texture — that view carries `flip_v`, so sprites drawn from it keep the same
orientation as `draw_render_target`. `set_blend_mode` records the accepted mode
in `Graphics.blend_mode`, so an effect can restore whatever the caller had
selected. Binding a render target unbinds the sampler texture first: a
framebuffer whose own texture is still readable is a feedback loop, and WebGL
drops those draws.

Custom shaders consume any subset of the renderer's standard vertex attributes:
`a_position`, `a_color`, `a_uv`, and `a_textured`. The engine supplies
`u_texture`, live frame time in `u_time`, and logical-pixel scaling in
`u_pixel_scale` when those uniforms are declared. `bind_shader` accepts a block
of `float_parameter`, vector-parameter, and integer-parameter values. It checks
every name and GLSL value shape through WebGL2 before selecting the program or
drawing, then caches uniform locations for that linked program revision.
`replace_shader` compiles and links a fresh program, atomically updates the
existing `Shader` handle, and advances its revision so the next bind resolves
fresh locations. A compile/link failure returns structured data and leaves the
prior program live; replacing an active program flushes and rebinds it before
deleting the old program. `use_shader`, `use_default_shader`, and the individual
uniform setters remain available for dynamic cases. Program and uniform changes
flush pending geometry, so one batch never observes two shader states.

For filled rectangles, `a_uv` spans `(0, 0)` at the top-left to `(1, 1)` at
the bottom-right even though `a_textured` is zero. This gives procedural
shaders stable shape-local coordinates that follow CPU-side transforms.
Sprites use their atlas UVs with `a_textured` set to one; other untextured
primitives currently receive zero UVs.

```walu
local graphics_module = require("waluau:engine/graphics")
local result = graphics:create_shader(vertex_source, pixel_source)
if result.ok then
    local bound = graphics:bind_shader(result.shader, {
        graphics_module.float_parameter("u_strength", 0.8),
        graphics_module.vec4_parameter("u_tint", 0.2, 0.8, 1.0, 1.0),
    })
    if bound.ok then
        graphics:fill_rectangle(24.0, 24.0, 96.0, 48.0)
    else
        print(bound.error.code .. ": " .. bound.error.message)
    end
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

## Particle systems

[`particles.walu`](particles.walu) is a buffered emitter with LÖVE's
`ParticleSystem` semantics, so tuning ported from a Love2D game means the same
thing here:

```walu
local particles = require("waluau:engine/particles")

local fire: particles.ParticleSystem = particles.new(600)
fire:set_position(160.0, 200.0)
fire:set_emission_rate(240.0)
fire:set_particle_lifetime(0.45, 1.25)
fire:set_direction(-math.pi * 0.5)
fire:set_spread(0.65)
fire:set_speed(50.0, 130.0)
fire:set_linear_acceleration(-24.0, -90.0, 24.0, -30.0)
fire:set_sizes({ 0.55, 1.35, 0.9, 0.1 })
fire:set_colors_hex({ "#fff6cc", "#ffc247", "#f4621e", "#3b0a0400" })
fire:set_blend_mode("add")

-- In update: fire:update(dt)
-- In draw:   fire:draw(graphics, 0.0, 0.0)
```

The emitter supports emission rate and emitter lifetime, `start`/`stop`/
`pause`/`reset`/`emit`, direction and spread, speed, linear, radial and
tangential acceleration, linear damping, a sideways sway, size and color curves
with size and color variation, rotation, spin with spin variation, relative
rotation, insert order
(`top`, `bottom`, `random`), spawn areas (`none`, `uniform`, `normal`,
`ellipse`, `borderellipse`, `borderrectangle`, each with a rotation angle and
optional outward aiming), `set_position` versus `move_to`, textures with
animated quads, and `clone`.

Three things differ from Love2D on purpose:

- **The buffer is a pool.** Every particle record is allocated when the buffer
  size is chosen and recycled after that, so a long-running emitter never grows
  its memory footprint. A full buffer drops new emissions, as in Love2D.
- **A texture is optional.** `set_draw_mode` selects `circle`, `square`,
  `point`, `spark` (a streak drawn along the velocity), or `texture`, so a
  system is useful before a game has any art. `set_shape_size` gives untextured
  particles their pixel size at scale 1.0.
- **Randomness is per system.** `set_seed` makes an emitter reproducible, and
  emitting never disturbs the `math.random` stream gameplay code draws from.

Two more are additions rather than differences, because Love2D has nothing to
port here:

- **`set_color_variation(variation)`** does for the color curve what
  `set_size_variation` does for sizes: each particle picks its own start and
  end on the curve at spawn, in `0..1`. Without it `color_at` is a pure
  function of age and every particle alive at the same age draws the same
  color; with it, `{"#292524", "#9a3412", "#78716c"}` at variation `1.0` is a
  crowd of flakes with their own tints rather than a fall that changes color
  together.
- **`set_sway_amplitude(minimum, maximum)`** and
  **`set_sway_frequency(minimum, maximum)`** rock a particle along the x axis,
  in pixels, at rocks per second, with a phase drawn per particle so a drift
  never rocks in unison. The three accelerations cannot do this — tangential
  acceleration gives a spiral, not a rock. Amplitude defaults to zero (no
  sway) and frequency to one, so an amplitude on its own already sways. The
  rock is a displacement, not a force: it leaves the velocity alone, so
  damping still settles a falling flake and a `spark` still points along its
  travel. Falling ash, drifting snow, a wobbling smoke column and floating
  embers are all this plus gravity.

`draw` restores the caller's color and blend mode, so a system never leaks its
own state into the rest of the frame. `particle_x`, `particle_size`,
`particle_angle`, `particle_color_alpha` and their siblings expose live
particles in draw order for games that would rather render them themselves.

[`fixtures/particles/`](../fixtures/particles/) is the gallery: nine scenes
(fire, fountain, explosions, smoke, weather, vortex, a pointer-following comet,
a sprite atlas built at runtime into a render target, and an ash fall of
swaying flakes each with their own ink) selected with the number keys, plus
[`sim.walu`](../fixtures/particles/sim.walu), which checks emission timing,
forces, curves and spawn areas without a canvas. The gallery is the
"Particle System" preset in the playground.

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
  per-game namespace. Slots are names rather than paths because a browser has
  no filesystem for a game to address, and the calls are asynchronous so a
  larger-capacity backing store than local storage can replace it without
  touching game source.

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
is the contract sample for image/font/audio readiness, safe failure, and save
reload behavior, written entirely against the service API rather than DOM
externs.

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
or wrongly typed requests. A declaration with a Waluau identifier in `name`
also appears in the generated `waluau:assets` module. Its `load()` operation
returns one bundle with nullable, opaque `ImageResource`, `FontResource`, and
`SoundResource` fields plus structured errors; named fonts also declare the
browser FontFace `family`. The bundle's `owner` releases every decoded source
through one idempotent lifecycle operation. Typed graphics and audio entry
points reject the wrong resource kind at compile time. Save namespaces never
consult this manifest. See
[`examples/game-project`](../examples/game-project/) for the complete layout
and build command.

## Intended API surface

The long-term public surface should remain small and subsystem-oriented:

- `engine`: lifecycle, configuration, quit/restart, errors, version/features
- `time`: fixed/variable timing and timers
- `input`: keyboard, pointer, touch, gamepad, text entry, focus
- `graphics`: canvas configuration, colors, transforms, shapes, text, images,
  atlases, sprite batches, meshes, render targets, blend/depth/stencil state,
  shaders and capability queries
- `particles`: pooled emitters, spawn areas, forces, size/color curves and
  textured or shape-drawn particles
- `audio`: decoded/streamed sources, playback state, buses and spatial audio
- `filesystem`: packaged assets and save data under logical names
- `physics`: optional 2D collision and rigid-body integration
- `math`: vectors, matrices, rectangles, interpolation, noise and randomness
- `system`: display information, clipboard, URLs and diagnostics

The API describes resources and commands rather than exposing DOM objects. That
is what keeps game code out of the host: a game says "draw this sprite", not
"bind this buffer". It is not a portability layer — WebGL2 is free to show
through wherever hiding it would cost performance, and the interface should be
designed around what WebGL2 does well.

## Capabilities required for a complete engine

The current language/runtime is sufficient for small shape-based browser games.
Reaching LÖVE-like usability requires these architectural capabilities:

| Area | Waluau/language requirement | Platform/runtime requirement |
| --- | --- | --- |
| Distribution | Stable package/virtual-module imports, API versioning and a standard project layout are available | Generated sibling JavaScript glue and typed fingerprinted asset manifests are available |
| Resources | Opaque host handles, nullable callbacks and host byte transfer are available | Async text/bytes/image/font/audio handles, decoded-image GPU upload, explicit lifetime, structured failures, and production packaging are available; caching remains |
| GPU graphics | Typed buffers, numeric/vector data, shader and uniform-friendly APIs | WebGL2 geometry, texture/sprite/glyph batching, render targets, game-provided vertex/pixel shader compilation, structured diagnostics, uniforms, and explicit shader lifetime are available |
| Input | Extensible event/value representation without a closed hard-coded record | Keyboard normalization, pointer/touch/gamepad polling, focus and fullscreen handling |
| Audio | Optional readiness callbacks and richer source state | Decoded effects and streamed music are available; buses, effects and richer mixing remain |
| Files | Stable project/package layout | Packaged fetch plus namespaced text/byte saves are available; larger-capacity save storage remains |
| Tooling | Source locations and protected error propagation across host callbacks | Project runner, asset pipeline, hot reload, debugger/profiler and distributable packaging |
| Performance | Predictable allocation, reusable buffers, broader numeric/vector operations | Batched submission, instancing, off-main-thread work where available, frame/memory profiling |

Several prerequisites already have repository issues, including 3D/GPU canvas
access (`waluau-9tvw`), generated JavaScript/Wasm glue (`waluau-884g`), dynamic
extern values (`waluau-lxdd`), host container marshalling (`waluau-utyc`), and
host-boundary error catching (`waluau-uvfk`). Engine-specific follow-ups cover
the stable package surface (`waluau-tpil`), GPU-backed renderer (`waluau-vt3k`),
and asset/audio/save services (`waluau-mi1t`). Beads remains the authoritative
source for priorities and completion status.

The renderer draws colored geometry, loaded textures, bitmap or custom-font
glyphs, atlas sprite batches, and render targets through WebGL2, using the
extern surface from `waluau-9tvw`. Games can compile and bind their own
vertex/pixel programs on that same batched stream.
