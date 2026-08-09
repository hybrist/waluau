// Compiles a Waluau example and runs it against the real top-level document.
//
// The playground's Run tab renders DOM output into a sandboxed iframe so the
// example cannot disturb the editor around it. The /output/* pages have no UI
// to protect, so they hand the page's own document to the module: the example
// owns the whole viewport, and its keyboard listeners see key presses without
// anything having to focus a frame first.

import {
  WALUAU_STRING_CONSTANTS_MODULE,
  WALUAU_MAIN_EXPORT,
  buildWaluauImports,
} from '../utils/wasm.js';
import '../dom-output.css';
import { loadWaluauWasm } from '../utils/waluauWasmModule.js';
import { withTypedAssetModule } from '../utils/typedAssets.js';

function reportError(error) {
  console.error('Example failed:', error);
  const message = typeof error === 'string' ? error : error?.message || String(error);
  const box = document.createElement('pre');
  box.id = 'example-error';
  box.setAttribute(
    'style',
    'margin:0;padding:1rem;white-space:pre-wrap;font-family:ui-monospace,monospace;color:#be123c;background:#fff1f2',
  );
  box.textContent = message;
  document.body.appendChild(box);
}

export async function renderExample({ files, entryFile, label, assetManifest = null }) {
  document.title = label;
  try {
    const compiler = await loadWaluauWasm();

    const compiled = compiler.compile_multi(
      withTypedAssetModule(files, assetManifest),
      entryFile,
    );
    const wasmBytes = new Uint8Array(compiled.wasm);
    const wasmModule = await WebAssembly.compile(wasmBytes, {
      builtins: ['js-string'],
      importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
    });

    let instance;
    const imports = buildWaluauImports(wasmModule, (msg) => console.log(msg), {
      wasmBytes,
      requiredImports: compiled.requiredImports,
      bytesConstants: compiled.bytesConstants,
      domOutputRoot: document,
      getWasmExports: () => instance.exports,
      onAsyncError: reportError,
      gameServices: { assetManifest },
    });
    instance = await WebAssembly.instantiate(wasmModule, imports);
    instance.exports[WALUAU_MAIN_EXPORT]?.();
  } catch (error) {
    reportError(error);
  }
}
