const WALUAU_STRING_CONSTANTS_MODULE = 'string_constants';
const WALUAU_IMPORT_MODULE = 'waluau';

function decodeBytesConstantsFromWasm(wasmBuffer) {
  const bytes = wasmBuffer instanceof Uint8Array ? wasmBuffer : new Uint8Array(wasmBuffer);
  let pos = 8;

  function readVaruint() {
    let result = 0;
    let shift = 0;
    while (pos < bytes.length) {
      const byte = bytes[pos++];
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) return result >>> 0;
      shift += 7;
    }
    throw new Error('truncated wasm leb128');
  }

  function readU32Le(offset) {
    return (
      bytes[offset] |
      (bytes[offset + 1] << 8) |
      (bytes[offset + 2] << 16) |
      (bytes[offset + 3] << 24)
    ) >>> 0;
  }

  while (pos < bytes.length) {
    const sectionId = bytes[pos++];
    const sectionLength = readVaruint();
    const sectionStart = pos;
    const sectionEnd = sectionStart + sectionLength;
    if (sectionEnd > bytes.length) throw new Error('wasm section extends past end of module');

    if (sectionId === 0) {
      const nameLen = readVaruint();
      const nameStart = pos;
      const nameEnd = nameStart + nameLen;
      const name = new TextDecoder().decode(bytes.subarray(nameStart, nameEnd));
      pos = nameEnd;
      if (name === 'waluau.bytc') {
        let offset = pos;
        const count = readU32Le(offset);
        offset += 4;
        const values = [];
        for (let i = 0; i < count; i++) {
          const len = readU32Le(offset);
          offset += 4;
          values.push(bytes.slice(offset, offset + len));
          offset += len;
        }
        return values;
      }
    }

    pos = sectionEnd;
  }

  return [];
}

function buildWaluauImports(wasmBuffer) {
  const bytesConstants = decodeBytesConstantsFromWasm(wasmBuffer);
  const asBytes = (value) => {
    if (value instanceof Uint8Array) return value;
    throw new Error(`Expected Uint8Array bytes value, got ${Object.prototype.toString.call(value)}`);
  };
  const waluauImports = new Proxy({}, {
    get(_target, prop) {
      const name = String(prop);
      if (name.startsWith('js_tostring_')) {
        return (value) => String(value);
      }
      if (name === 'print' || name === 'js_log') {
        return () => {};
      }
      if (name === 'bytes_literal') {
        return (index) => {
          const literal = bytesConstants[index];
          if (!literal) throw new Error(`Unknown bytes literal index ${index}`);
          return literal.slice();
        };
      }
      if (name === 'bytes_get') {
        return (value, index) => {
          const bytes = asBytes(value);
          if (index < 0 || index >= bytes.length) throw new Error(`bytes index out of bounds: ${index}`);
          return bytes[index];
        };
      }
      if (name === 'bytes_len') {
        return (value) => asBytes(value).length;
      }
      if (name === 'bytes_concat') {
        return (left, right) => {
          const a = asBytes(left);
          const b = asBytes(right);
          const merged = new Uint8Array(a.length + b.length);
          merged.set(a, 0);
          merged.set(b, a.length);
          return merged;
        };
      }
      if (name === 'bytes_eq') {
        return (left, right) => {
          const a = asBytes(left);
          const b = asBytes(right);
          if (a.length !== b.length) return 0;
          for (let i = 0; i < a.length; i++) {
            if (a[i] !== b[i]) return 0;
          }
          return 1;
        };
      }
      if (name === 'bytes_compare') {
        return (left, right) => {
          const a = asBytes(left);
          const b = asBytes(right);
          const len = Math.min(a.length, b.length);
          for (let i = 0; i < len; i++) {
            if (a[i] < b[i]) return -1;
            if (a[i] > b[i]) return 1;
          }
          if (a.length < b.length) return -1;
          if (a.length > b.length) return 1;
          return 0;
        };
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
  const imports = buildWaluauImports(wasmBuffer);
  await WebAssembly.instantiate(wasmBuffer, imports, {
    builtins: ['js-string'],
    importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
  });
}
