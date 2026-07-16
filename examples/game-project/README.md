# Minimal Waluau game project

This directory is the supported version-1 project layout: an entry
`main.walu`, project-owned relative modules beside it, and optional `assets/`
and `dist/` directories. Engine code is imported from the compiler-embedded
package rather than copied into the project.

Build the project with an installed compiler:

```sh
mkdir -p dist
waluau main.walu -o dist/game.wasm
```

From a Waluau repository checkout, the equivalent command is:

```sh
mkdir -p examples/game-project/dist
cargo run -p waluau-cli -- examples/game-project/main.walu \
  -o examples/game-project/dist/game.wasm
```

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

The compiler currently emits the `.wasm` artifact. The standard sibling
JavaScript launcher belongs to generated-glue issue `waluau-884g`; package
resolution is implemented in the same compiler linkers so that launcher will
not need a second package loader.
