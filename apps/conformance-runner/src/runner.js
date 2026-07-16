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
    const useCompilerMetadata = options.compilerMetadata !== false;
    const imports = buildWaluauImports(wasmModule, undefined, {
      ...options,
      wasmBytes: wasmBuffer,
      requiredImports: useCompilerMetadata ? output.requiredImports : undefined,
      bytesConstants: useCompilerMetadata ? output.bytesConstants : undefined,
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
      requiredImports: output.requiredImports,
      bytesConstants: output.bytesConstants,
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

export async function compileAndRunGeneratedGlue(files, entryFile = '/main.walu', options = {}) {
  const compiler = await import('./waluau-wasm/waluau_wasm.js');
  await compiler.default();
  const output = compiler.compile_multi(files, entryFile);
  const blobUrl = URL.createObjectURL(new Blob([output.jsGlue], { type: 'text/javascript' }));
  const iframe = document.createElement('iframe');
  iframe.style.display = 'none';
  document.body.appendChild(iframe);
  const domOutputRoot = iframe.contentDocument;
  domOutputRoot.open();
  domOutputRoot.write('<!doctype html><html><body></body></html>');
  domOutputRoot.close();

  try {
    const glue = await import(/* @vite-ignore */ blobUrl);
    const loaded = await glue.run({
      wasm: new Uint8Array(output.wasm),
      assetBaseUrl: new URL('./', window.location.href),
      ...options,
      createImports: (context) => buildWaluauImports(null, undefined, {
        ...options,
        requiredImports: context.requiredImports,
        bytesConstants: context.bytesConstants,
        domOutputRoot,
        getWasmExports: context.getWasmExports,
      }),
    });
    return {
      ...loaded,
      root: domOutputRoot.body,
      cleanup: () => iframe.remove(),
    };
  } catch (error) {
    iframe.remove();
    throw error;
  } finally {
    URL.revokeObjectURL(blobUrl);
  }
}
