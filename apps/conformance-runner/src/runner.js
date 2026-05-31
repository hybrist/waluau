const WALUAU_STRING_CONSTANTS_MODULE = 'string_constants';
const WALUAU_IMPORT_MODULE = 'waluau';

function buildWaluauImports() {
  const waluauImports = new Proxy({}, {
    get(_target, prop) {
      const name = String(prop);
      if (name.startsWith('js_tostring_')) {
        return (value) => String(value);
      }
      if (name === 'print' || name === 'js_log') {
        return () => {};
      }
      return () => {
        throw new Error(`Unsupported waluau import: ${name}`);
      };
    },
  });
  return {
    [WALUAU_IMPORT_MODULE]: waluauImports,
  };
}

export async function compileAndInstantiate(files, entryFile = '/main.walu') {
  const module = await import('./waluau-wasm/waluau_wasm.js');
  await module.default();
  const output = module.compile_multi(files, entryFile);
  const wasmBuffer = new Uint8Array(output.wasm);
  const imports = buildWaluauImports();
  await WebAssembly.instantiate(wasmBuffer, imports, {
    builtins: ['js-string'],
    importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
  });
}
