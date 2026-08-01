# @waluau/storybook

Storybook framework for Waluau. Stories are `.walu` files drawn by the engine;
Storybook is the real thing around them — its sidebar, index, URLs, toolbar,
viewport and addons — with a renderer that puts one engine-drawn scene on the
canvas at a time.

```js
// .storybook/main.js
import { shaderSources } from '../shader-sources.js';

export default {
  stories: ['../src/**/*.stories.walu'],
  framework: {
    name: '@waluau/storybook',
    options: {
      // Passed to @waluau/vite-plugin, so stories compile with the same
      // packaged assets and external shader sources as the game.
      waluau: { manifest: 'waluau.assets.json', shaderSources },
    },
  },
};
```

```bash
storybook dev -p 6006
```

## Writing stories

A story file declares one or more stories and publishes them. The file's top
level is where they are declared, the same way a Component Story Format module
exports them:

```walu
local storybook = require("waluau:engine/storybook")
local engine = require("waluau:engine")
local render = require("./render")

local function draw_face_up(graphics: engine.Graphics, alpha: f64): unit
    render.draw_card(graphics, card, 40.0, 40.0)
end

local function draw_face_down(graphics: engine.Graphics, alpha: f64): unit
    render.draw_card_back(graphics, 40.0, 40.0)
end

storybook.publish({
    storybook.story("Face up", { draw = draw_face_up }),
    storybook.story("Face down", { draw = draw_face_down }),
})
```

A story's scene is `draw` plus whatever else it needs: `load` (run each time
the story is mounted), `update`, `keypressed`, `keyreleased`, `mousepressed`,
`mousereleased`, `mousemoved`, and `width`/`height`/`background` for a story the
default 960x600 canvas does not frame. Pointer coordinates arrive in the
story's own space, because the canvas is the story's own.

Names are read out of the source to build Storybook's index, so
`storybook.story("<name>", ...)` needs a literal name; the sidebar entry and the
mountable story are then the same name by construction. The story's title comes
from the file's path, like any other Storybook file.

Shared setup — loading a font, an atlas, a set of shaders — belongs at the top
level of the story file, where every story in it can use it. All the stories in
one file share a canvas and a WebGL context; moving between them resizes that
canvas rather than building a new one, so what the file loaded once stays
valid.

## What the framework does

- `core.builder` is `@storybook/builder-vite`, and `viteFinal` adds
  `@waluau/vite-plugin`, configured from `framework.options.waluau`. Storybook
  owns this Vite configuration: an app's own `vite.config.js` copy of the plugin
  is replaced, because its full-screen game viewport would stretch every story
  over Storybook's layout.
- `experimental_indexers` indexes `*.stories.walu` by reading the story names
  out of the file.
- The Vite plugin turns each `*.stories.walu` into a CSF module: one named
  export per story, all mounting through the file's compiled Wasm module.
- The renderer mounts a story into the element Storybook hands it and returns a
  teardown that stops the session.

Docs blocks (`@storybook/addon-docs`) are not supported: a story is a canvas,
and there is no source snippet or args table to generate from it. Controls are
likewise empty — stories have no args.
