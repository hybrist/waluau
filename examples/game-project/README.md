# Minimal Waluau game project

This directory is the supported version-1 project layout: an entry
`main.walu`, project-owned relative modules beside it, and optional `assets/`
and `dist/` directories. Engine code is imported from the compiler-embedded
package rather than copied into the project.

Build the project with an installed compiler:

```sh
mkdir -p dist
waluau main.walu -o dist/game.wasm --emit-js
```

From a Waluau repository checkout, the equivalent command is:

```sh
mkdir -p examples/game-project/dist
cargo run -p waluau-cli -- examples/game-project/main.walu \
  -o examples/game-project/dist/game.wasm --emit-js
```

This writes `game.wasm` and its ES-module sibling `game.js`. The generated
module exports `requiredImports`, `bytesConstants`, `wasmUrl`, `instantiate`,
and `run`. It resolves `game.wasm` relative to `import.meta.url` and never
reflects on or parses the Wasm import section:

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
an `import.meta.url`-relative base URL without changing module imports.

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

For in-memory consumers, the compiler result exposes the same JavaScript as
`jsGlue` plus `requiredImports` and `bytesConstants`. Existing hosts may keep
using Wasm import reflection or byte parsing as a compatibility fallback, but
new integrations should pass the compiler metadata to their host factory.
