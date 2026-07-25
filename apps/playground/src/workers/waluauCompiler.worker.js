import { loadWaluauWasm } from '../utils/waluauWasmModule.js';

const compilerSession = loadWaluauWasm().then((module) => ({
  module,
  languageServer: new module.WaluauLanguageServer(),
}));

function stringifyError(error) {
  return typeof error === 'string' ? error : error?.message || String(error);
}

function transferableBuffers(result) {
  const buffers = new Set();
  const output = result?.output ?? result;
  if (output?.wasm?.buffer instanceof ArrayBuffer) {
    buffers.add(output.wasm.buffer);
  }
  for (const bytes of output?.bytesConstants ?? []) {
    if (bytes?.buffer instanceof ArrayBuffer) {
      buffers.add(bytes.buffer);
    }
  }
  return [...buffers];
}

self.onmessage = async ({ data }) => {
  const { id, type, payload } = data;
  try {
    const { module, languageServer } = await compilerSession;
    let result;
    if (type === 'initialize') {
      result = true;
    } else if (type === 'analyzeProject') {
      result = languageServer.analyzeProject(payload.files, payload.entryFile);
    } else if (type === 'compileProject') {
      result = module.compile_multi(payload.files, payload.entryFile);
    } else if (type === 'lspRequest') {
      if (payload.document != null) {
        languageServer.updateDocument(payload.document.path, payload.document.text);
      }
      const outgoing = languageServer.handleMessage(JSON.stringify({
        jsonrpc: '2.0',
        id,
        method: payload.method,
        params: payload.params,
      }));
      result = null;
      for (const message of outgoing) {
        const parsed = JSON.parse(message);
        if (parsed.id === id) {
          result = parsed.result ?? null;
          break;
        }
      }
    } else {
      throw new Error(`Unknown compiler worker request: ${type}`);
    }
    self.postMessage({ id, ok: true, result }, transferableBuffers(result));
  } catch (error) {
    self.postMessage({ id, ok: false, error: stringifyError(error) });
  }
};
