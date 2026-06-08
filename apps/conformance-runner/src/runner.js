import {
  WALUAU_STRING_CONSTANTS_MODULE,
  buildWaluauImports,
} from '../../playground/src/utils/wasm.js';

export async function compileAndInstantiate(files, entryFile = '/main.walu') {
  await compileAndInstantiateWithExports(files, entryFile);
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
    const imports = buildWaluauImports(wasmBuffer, undefined, { domOutputRoot });
    const result = await WebAssembly.instantiate(wasmBuffer, imports, {
      builtins: ['js-string'],
      importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
    });
    return result.instance.exports;
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
    const exports = await compileAndInstantiateWithExports(files, entryFile, { domOutputRoot });
    return {
      exports,
      root: domOutputRoot.body,
      cleanup: () => {
        iframe.remove();
      },
    };
  } catch (err) {
    iframe.remove();
    throw err;
  }
}
