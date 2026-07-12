import {
  WALUAU_STRING_CONSTANTS_MODULE,
  buildWaluauImports,
  WALUAU_IMPORT_MODULE,
  WALUAU_MAIN_EXPORT,
} from '../../playground/src/utils/wasm.js';

export async function compileAndInstantiate(files, entryFile = '/main.walu', options = {}) {
  await compileAndInstantiateWithExports(files, entryFile, options);
}

export async function compileAndInstantiateWithExports(files, entryFile = '/main.walu', options = {}) {
  let tempIframe = null;
  let domOutputRoot = options.domOutputRoot;

  if (!domOutputRoot) {
    tempIframe = document.createElement('iframe');
    tempIframe.style.display = 'none';
    document.body.appendChild(tempIframe);
    domOutputRoot = tempIframe.contentDocument;

    domOutputRoot.open();
    domOutputRoot.write('<!doctype html><html><body></body></html>');
    domOutputRoot.close();
  }

  try {
    const module = await import('./waluau-wasm/waluau_wasm.js');
    await module.default();
    const output = module.compile_multi(files, entryFile);
    const wasmBuffer = new Uint8Array(output.wasm);
    const wasmModule = await WebAssembly.compile(wasmBuffer, {
      builtins: ['js-string'],
      importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
    });

    let instance;
    const imports = buildWaluauImports(wasmModule, undefined, {
      ...options,
      wasmBytes: wasmBuffer,
      domOutputRoot,
      getWasmExports: () => instance.exports,
    });

    instance = await WebAssembly.instantiate(wasmModule, imports);
    instance.exports[WALUAU_MAIN_EXPORT]?.();
    return instance.exports;
  } finally {
    if (tempIframe) {
      tempIframe.remove();
    }
  }
}

export async function compileAndInstantiateWithDom(files, entryFile = '/main.walu') {
  const iframe = document.createElement('iframe');
  iframe.style.display = 'none';
  document.body.appendChild(iframe);
  const domOutputRoot = iframe.contentDocument;

  domOutputRoot.open();
  domOutputRoot.write('<!doctype html><html><body></body></html>');
  domOutputRoot.close();

  try {
    const module = await import('./waluau-wasm/waluau_wasm.js');
    await module.default();
    const output = module.compile_multi(files, entryFile);
    const wasmBuffer = new Uint8Array(output.wasm);
    const wasmModule = await WebAssembly.compile(wasmBuffer, {
      builtins: ['js-string'],
      importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
    });

    let instance;
    const imports = buildWaluauImports(wasmModule, undefined, {
      wasmBytes: wasmBuffer,
      domOutputRoot,
      getWasmExports: () => instance.exports,
    });

    instance = await WebAssembly.instantiate(wasmModule, imports);
    instance.exports[WALUAU_MAIN_EXPORT]?.();

    const storage = imports[WALUAU_IMPORT_MODULE]['Window.get/localStorage'](domOutputRoot.defaultView);

    return {
      exports: instance.exports,
      root: domOutputRoot.body,
      storage,
      cleanup: () => {
        iframe.remove();
      },
    };
  } catch (err) {
    iframe.remove();
    throw err;
  }
}
