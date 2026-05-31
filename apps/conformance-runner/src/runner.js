const WALUAU_IMPORT_MODULE = 'waluau';
const WALUAU_STRING_SECTION = 'waluau.strc';

function readU32Le(bytes, offset) {
  return (
    bytes[offset] |
    (bytes[offset + 1] << 8) |
    (bytes[offset + 2] << 16) |
    (bytes[offset + 3] << 24)
  ) >>> 0;
}

function parseWaluauStringSection(buffer) {
  const bytes = new Uint8Array(buffer);
  let pos = 8;
  while (pos < bytes.length) {
    const sectionId = bytes[pos++];
    let sectionLen = 0;
    let shift = 0;
    while (true) {
      const byte = bytes[pos++];
      sectionLen |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) break;
      shift += 7;
    }
    const sectionEnd = pos + sectionLen;
    if (sectionId === 0) {
      let nameLen = 0;
      shift = 0;
      while (true) {
        const byte = bytes[pos++];
        nameLen |= (byte & 0x7f) << shift;
        if ((byte & 0x80) === 0) break;
        shift += 7;
      }
      const name = new TextDecoder().decode(bytes.subarray(pos, pos + nameLen));
      pos += nameLen;
      if (name === WALUAU_STRING_SECTION) {
        const data = bytes.subarray(pos, sectionEnd);
        const count = readU32Le(data, 0);
        const strings = [];
        let offset = 4;
        for (let i = 0; i < count; i++) {
          const len = readU32Le(data, offset);
          offset += 4;
          strings.push(new TextDecoder().decode(data.subarray(offset, offset + len)));
          offset += len;
        }
        return strings;
      }
    }
    pos = sectionEnd;
  }
  return [];
}

function buildWaluauImports(strings) {
  return {
    [WALUAU_IMPORT_MODULE]: {
      js_string_const: (index) => strings[index] ?? '',
      js_string_eq: (left, right) => (left === right ? 1 : 0),
      js_string_concat: (left, right) => `${left}${right}`,
      print: () => {},
      js_tostring_i32: (value) => `${value | 0}`,
      js_tostring_u32: (value) => `${value >>> 0}`,
      js_tostring_i64: (value) => `${value}`,
      js_tostring_u64: (value) => `${BigInt.asUintN(64, value)}`,
      js_tostring_f32: (value) => `${value}`,
      js_tostring_f64: (value) => `${value}`,
      js_tostring_bool: (value) => `${value !== 0}`,
    },
  };
}

export async function compileAndInstantiate(files, entryFile = '/main.walu') {
  const module = await import('./waluau-wasm/waluau_wasm.js');
  await module.default();
  const output = module.compile_multi(files, entryFile);
  const wasmBuffer = new Uint8Array(output.wasm);
  const strings = parseWaluauStringSection(wasmBuffer);
  const imports = buildWaluauImports(strings);
  await WebAssembly.instantiate(wasmBuffer, imports);
}
