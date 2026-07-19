# Minimal Waluau game project

This directory is the supported version-1 project layout: an entry
`main.walu`, project-owned relative modules beside it, and optional `assets/`
and `dist/` directories. Engine code is imported from the compiler-embedded
package rather than copied into the project.

Build the project with an installed compiler:

```sh
mkdir -p dist
waluau main.walu -o dist/game.wasm --emit-js \
  --manifest waluau.assets.json
```

From a Waluau repository checkout, the equivalent command is:

```sh
mkdir -p examples/game-project/dist
cargo run -p waluau-cli -- examples/game-project/main.walu \
  -o examples/game-project/dist/game.wasm --emit-js \
  --manifest examples/game-project/waluau.assets.json
```

Run the same project through the Vite plugin from this repository:

```sh
pnpm dev:game-project
```

Move the square with the right arrow, then edit `main.walu`. Its position is
captured and restored automatically when the snapshot schema remains
compatible.

This writes `game.wasm`, its ES-module sibling `game.js`, and fingerprinted
copies of every declared asset. The version-1 manifest accepts exactly
`text`, `bytes`, `image`, `font`, and `audio` entries. Paths are normalized,
project-relative logical names; schemes, absolute paths, traversal, encoded
traversal, duplicate declarations, missing files, and unknown types are build
errors.

```json
{
  "version": 1,
  "assets": [
    { "path": "assets/welcome.txt", "type": "text" },
    { "path": "assets/player.svg", "type": "image" }
  ]
}
```

The generated module exports `requiredImports`, `bytesConstants`, `wasmUrl`, `instantiate`,
`run`, `assetBaseUrl`, and `assetManifest`. Manifest values contain the
fingerprinted import-meta-relative URL and declared type, while game code keeps
requesting the original logical path. It resolves `game.wasm` relative to
`import.meta.url` and never reflects on or parses the Wasm import section:

```js
import { run } from './dist/game.js';
import { createBrowserImports } from './browser-host.js';

await run({
  createImports: ({ requiredImports, bytesConstants, getWasmExports }) =>
    createBrowserImports({ requiredImports, bytesConstants, getWasmExports }),
});
```

`createImports` may return a broad host namespace; generated glue selects only
the compiler-known imports used by the module. It also receives `wasmUrl`,
`assetBaseUrl`, `assetManifest`, and `hostOptions`. Asset metadata is separate
from the Wasm ABI: a packager can pass a normalized logical-path manifest and
an `import.meta.url`-relative base URL without changing module imports. Missing
logical entries and requests through the wrong typed loader resolve to
structured `undeclared_asset` and `wrong_asset_type` resource errors. Save
slots remain separately validated and namespaced; they never consult this
read-only manifest.

The browser conformance suite builds this source as the only project file,
instantiates it with the standard Waluau browser host, and verifies its first
rendered frame:

```sh
pnpm --filter conformance-runner test:browser
```

`waluau:engine` tracks the current stable major version. Applications that
need reproducible major-version selection may import `waluau:engine/v1`.
Subsystems remain available as `waluau:engine/input`,
`waluau:engine/graphics`, `waluau:engine/time`, and their `/v1/` forms.

The sample also uses the Vite plugin's development-only `waluau:engine/hot`
registration.
Its snapshot prefix is an application-owned schema marker; changing or
rejecting that marker intentionally causes a full reload. These transient
snapshots are never stored and are separate from engine save slots.

For in-memory consumers, the compiler result exposes the same JavaScript as
`jsGlue` plus `requiredImports` and `bytesConstants`. Existing hosts may keep
using Wasm import reflection or byte parsing as a compatibility fallback, but
new integrations should pass the compiler metadata to their host factory.
