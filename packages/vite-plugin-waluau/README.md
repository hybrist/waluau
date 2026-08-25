# @waluau/vite-plugin

This plugin makes `.walu` files ordinary Vite modules. Importing one compiles
the Waluau project rooted at that file, supplies the standard browser imports,
starts the game, and exports its loading promise as both `game` and the default
export. The generated game owns the entire viewport by default.

```js
// vite.config.js
import { defineConfig } from 'vite';
import { waluau } from '@waluau/vite-plugin';

export default defineConfig({
  plugins: [waluau()],
});
```

Point a normal module script at the Waluau source:

```html
<script type="module" src="/src/main.walu"></script>
```

The same file can be imported from JavaScript with
`import game from './main.walu'`. Set `fullScreen: false` to embed the game
without the viewport styles. Outside the Waluau repository, install the
`waluau` compiler binary or pass a custom `compiler` command.

## Development hot replacement

When Vite recompiles a running game, the plugin can replace its Wasm instance
without reloading the page. A game opts in by registering three closures:

```walu
local engine = require("waluau:engine")
local hot = require("waluau:engine/hot")
local session: engine.Session? = nil
local score: i32 = 0

session = engine.start(config, callbacks)

hot.register({
    snapshot = function(): string
        return "my-game:v1:" .. tostring(score)
    end,
    restore = function(snapshot: string): bool
        const prefix: string = "my-game:v1:"
        if string.sub(snapshot, 1, #prefix) ~= prefix then return false end
        local restored: f64? = tonumber(string.sub(snapshot, #prefix + 1))
        if restored == nil then return false end
        score = (restored::f64)::i32
        return true
    end,
    dispose = function(): unit
        local running: engine.Session? = session
        if running ~= nil then
            running.suspend()
            session = nil
        end
    end,
})
```

The plugin captures the old snapshot, disposes its browser loop, starts the
new Wasm module, and passes the snapshot to the registered restore closure.
The snapshot must be a self-contained string; include a schema or build marker
and return `false` when the new code cannot safely interpret it. Missing
registration, capture or startup errors, non-string snapshots, and rejected
or failed restores all fall back to a full page reload.

This is transient development state. The plugin never writes it to storage and
does not promise long-term compatibility. Use the engine save-data service for
player saves. Production builds accept the registration as an inert no-op.

## External shader sources

The `shaderSources` option gives standalone shader files stable application
names. Paths are resolved from the Vite project root.

```js
export default defineConfig({
  plugins: [
    waluau({
      shaderSources: {
        'effects.vertex': 'src/shaders/effects.vert',
        'effects.fire': 'src/shaders/fire.frag',
      },
    }),
  ],
});
```

Vite imports these files as raw strings in development and production, so both
modes use the same source contract. Do not also list shader files in
`waluau.assets.json`; shader sources are synchronous build inputs, not runtime
resource loads.

Waluau code opens each name through `waluau:engine/shader_sources` and polls at
a frame boundary. The first poll returns the initial text with `changed =
true`. Each later Vite edit advances that source's revision and is returned
once, without invoking the Waluau compiler or replacing the running Wasm
instance.

```walu
local shader_sources = require("waluau:engine/shader_sources")

local vertex = shader_sources.open("effects.vertex")
local pixel = shader_sources.open("effects.fire")
local vertex_text: string = ""
local pixel_text: string = ""
local shader: graphics_module.Shader? = nil
local shader_error: graphics_module.GraphicsError? = nil

function refresh_shader(graphics: graphics_module.Graphics): unit
    local vertex_update = vertex:poll()
    local pixel_update = pixel:poll()
    if not vertex_update.ok or not pixel_update.ok then
        local failure = if not vertex_update.ok then vertex_update.error else pixel_update.error
        shader_error = { code = failure.code, message = failure.message }
        return
    end
    if vertex_update.changed then vertex_text = vertex_update.source end
    if pixel_update.changed then pixel_text = pixel_update.source end
    if not vertex_update.changed and not pixel_update.changed then return end

    local current = shader
    local result: graphics_module.ShaderResult
    if current == nil then
        result = graphics:create_shader(vertex_text, pixel_text)
    else
        result = graphics:replace_shader(current::graphics_module.Shader, vertex_text, pixel_text)
    end
    if result.ok then
        shader = result.shader
        shader_error = nil
    else
        -- This revision is consumed, so it is not retried every frame. The
        -- next edit advances the revision and retries. replace_shader keeps
        -- the last valid program live.
        shader_error = result.error
    end
end
```

`Source:poll()` returns `{ changed, ok, source, revision, error }`. A missing
plugin configuration or unknown name produces a structured `missing` error on
the first poll rather than a Wasm instantiation failure. Invalid GLSL remains
an application-level `vertex_compile`, `pixel_compile`, or `link` diagnostic
from `Graphics:replace_shader`; a later valid edit can recover using the same
shader handle. Released shader handles return a `released` error.

Each raw dependency has its own Vite accept callback. Editing one of several
shader files updates only its configured name; the `.walu` module's normal
whole-game HMR path is not entered.

The plugin keeps one stateful compiler process alive for the Vite lifecycle.
Repeated builds reuse the compiler session's module parse cache instead of
starting `waluau` (or `cargo run`) for every edit. Changes to embedded engine,
builtin, extern, or Rust compiler sources restart that process deliberately so
the next build incorporates the new compiler inputs. Inside this repository,
the host uses Cargo's optimized release profile so development rebuilds do not
pay debug-build execution costs. A custom compiler command
keeps its historical one-process-per-build behavior unless configured with
`compiler: { command, args, persistent: true }`; persistent commands must
implement the `waluau --server` newline-delimited JSON protocol.

## Packaged assets

Pass a version-1 `waluau.assets.json` file through the `manifest` option to
compile and serve typed packaged assets during development and production
builds. The path is resolved from the Vite app root and watched for changes.

```js
export default defineConfig({
  plugins: [waluau({ manifest: 'waluau.assets.json' })],
});
```

The manifest's asset paths are relative to the manifest itself and remain the
logical paths requested by Waluau source. Give image, font, and audio entries a
Waluau identifier in `name` to expose them through `require("waluau:assets")`;
named fonts additionally require their browser FontFace `family`. The generated
module loads all named entries as one typed, explicitly owned bundle. See the
compiler's asset-manifest documentation for the supported asset types.

## Testing with vitest

Files named `*.test.walu` become vitest test modules instead of games: the
plugin compiles them the same way, then registers their suites with vitest
during collection. Pull in the test API with `require("waluau:vitest")` and
write busted-style suites with vitest-style matchers:

```lua
require("waluau:vitest")

function add(a: i32, b: i32): i32
    return a + b
end

describe("arithmetic", function(): unit
    before_each(function(): unit
    end)

    it("adds integers", function(): unit
        expect(add(2, 2)):toBe(4)
        expect(0.1 + 0.2):toBeCloseTo(0.3)
        expect("hello walu"):toContain("walu")
        expect(add(1, 1) == 2):toBeTruthy()
    end)
end)
```

The API (declared in `externs/vitest.walu`): `describe`, `it`, `test`,
`xdescribe`/`xit` (skip), `todo(name)`, the hooks `before_each`,
`after_each`, `before_all`, `after_all`, and `expect(value)` whose matcher
set follows the value's static type (numbers: `toBe`, `notToBe`,
`toBeCloseTo`, `toBeGreaterThan[OrEqual]`, `toBeLessThan[OrEqual]`; strings:
`toBe`, `notToBe`, `toContain`, `notToContain`, `toHaveLength`; booleans:
`toBe`, `toBeTruthy`, `toBeFalsy`; externs: identity `toBe`/`notToBe`;
enums: ordinal `toBe`/`notToBe`). Every expectation also chains the `:not`
modifier (vitest's `.not`), negating the matcher that follows:

```lua
enum Suit { clubs, diamonds, hearts, spades }

it("compares enums and negates matchers", function(): unit
    expect(Suit.hearts):toBe(Suit.hearts)
    expect(Suit.hearts):not:toBe(Suit.spades)
    expect(add(2, 2)):not:toBe(5)
end)
```

Enum values cross the host boundary as their i32 ordinals: matchers compare
ordinals (failure messages show the numbers), and because declared host
functions cannot be generic, `expect` accepts any enum — comparing values
of two different enums type-checks and compares raw ordinals.
Failed Waluau `assert(cond, msg)` calls surface as readable vitest failures
with correct file/line info.

Wire vitest (browser mode) into the same vite config and include the test
files:

```js
export default defineConfig({
  plugins: [waluau()],
  test: {
    include: ['src/**/*.test.walu'],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: [{ browser: 'chromium' }],
    },
  },
});
```

`apps/ante` is wired up this way. The bridge itself lives in
`@waluau/vite-plugin/testing`; its meta-tests (and a browser-compiled
variant used by the conformance runner) live in `apps/conformance-runner`.

## Stories for Storybook

Files named `*.stories.walu` become Component Story Format modules instead of
games: the plugin compiles them the same way, reads the published story names
out of the source, and generates one named CSF export per story. Each export
mounts its story through `@waluau/vite-plugin/storybook`, which bridges
`engine/storybook.walu`'s registration imports onto the compiled module.

```walu
local storybook = require("waluau:engine/storybook")
local engine = require("waluau:engine")

local function draw_face_up(graphics: engine.Graphics, alpha: f64): unit
end

storybook.publish({
    storybook.story("Face up", { draw = draw_face_up }),
})
```

The plugin does not run Storybook. `@waluau/storybook` is the framework that
does: it adds this plugin to Storybook's Vite config, indexes the same story
names for the sidebar, and renders them. See that package's README.
