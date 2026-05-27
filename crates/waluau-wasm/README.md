# waluau-wasm

WebAssembly bindings for the Waluau compiler, generated with [wasm-bindgen](https://rustwasm.github.io/wasm-bindgen/).

## Build

The playground Vite plugin (`apps/playground/vite-plugin-waluau-wasm.js`) builds this crate automatically during `pnpm dev` and `pnpm build:playground`. No separate wasm build step is required.

Generated JS glue is written to `apps/playground/src/waluau-wasm/`.

## JavaScript API

```javascript
import init, { compile } from './waluau-wasm/waluau_wasm.js';

await init();

const { ir, wat, wasm, requiresWasmGc } = compile(sourceCode);
// wasm is a Uint8Array of the compiled module bytes
```

When using the `--target web` output consumed by Vite, call the default export once to initialize the module, then call `compile`:

```javascript
const { default: init, compile } = await import('./waluau-wasm/waluau_wasm.js');
await init();
const { ir, wat, wasm, requiresWasmGc } = compile(sourceCode);
```

### `compile(source: string)`

Compiles Waluau source and returns:

| Field | Type | Description |
|-------|------|-------------|
| `ir` | `string` | Textual IR dump |
| `wat` | `string` | WebAssembly text format |
| `wasm` | `Uint8Array` | Compiled Wasm bytes |
| `requiresWasmGc` | `boolean` | `true` when the module uses array reference types and needs a Wasm GC-capable engine |

Throws a string error message when parsing, type-checking, IR construction, or codegen fails.

## Breaking changes from the manual FFI

The previous hand-rolled ABI exported low-level helpers that callers had to orchestrate:

- `alloc(size) -> pointer`
- `dealloc(pointer, size)`
- `compile(pointer, len) -> c-string pointer`
- `free_string(pointer)`

Callers copied UTF-8 bytes into Wasm linear memory, read a null-terminated result string, and freed both allocations manually.

The wasm-bindgen surface replaces that with a single `compile(source)` call. There is no public allocation API anymore. Success and error handling use normal JavaScript values instead of the `Success:\n` / `Error:\n` prefixed C strings.

The compile surface remains wasm-bindgen based and now returns `{ ir, wat, wasm, requiresWasmGc }`.
