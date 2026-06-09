export const WALUAU_STRING_CONSTANTS_MODULE = 'string_constants';
export const WALUAU_IMPORT_MODULE = 'waluau';
// Must match waluau_codegen_wasm::host::HOST_IMPORT_COUNT
export const WALUAU_HOST_IMPORT_COUNT = 19;
const PROMISE_RESUME_TRAMPOLINE_EXPORT = '__waluau_resume_promise_await';
const PROMISE_RESET_ACTIVE_EXPORT = '__waluau_reset_active_coroutine';

let printCaptureCallback = null;
const domEventListeners = new WeakMap();

function rememberDomEventListener(target, type, listener) {
  let records = domEventListeners.get(target);
  if (!records) {
    records = new Set();
    domEventListeners.set(target, records);
  }
  records.add({ type, listener });
}

function childrenOfDomNode(node) {
  return Array.from(node?.children ?? node?.childNodes ?? []);
}

export function cleanupDomEventListeners(node) {
  if (!node || typeof node !== 'object') return;
  for (const child of childrenOfDomNode(node)) cleanupDomEventListeners(child);
  const records = domEventListeners.get(node);
  if (!records) return;
  for (const { type, listener } of records) {
    node.removeEventListener(type, listener);
  }
  records.clear();
  domEventListeners.delete(node);
}

export function decodeBytesConstantsFromWasm(wasmModule) {
  const section = WebAssembly.Module.customSections(wasmModule, 'waluau.bytc')[0];
  if (!section) return [];

  const bytes = new Uint8Array(section);
  let pos = 0;

  function readU32Le(offset) {
    return (
      bytes[offset] |
      (bytes[offset + 1] << 8) |
      (bytes[offset + 2] << 16) |
      (bytes[offset + 3] << 24)
    ) >>> 0;
  }

  const count = readU32Le(pos);
  pos += 4;
  const values = [];
  for (let i = 0; i < count; i++) {
    const len = readU32Le(pos);
    pos += 4;
    values.push(bytes.slice(pos, pos + len));
    pos += len;
  }
  return values;
}

export function parseBytesInput(valStr) {
  const trimmed = valStr.trim();
  if (trimmed.startsWith('[')) {
    const parsed = JSON.parse(trimmed);
    if (!Array.isArray(parsed)) throw new Error('bytes input must be a JSON array');
    return new Uint8Array(parsed.map((value) => {
      const n = Number(value);
      if (!Number.isInteger(n) || n < 0 || n > 255) {
        throw new Error('bytes values must be integers in the range 0..255');
      }
      return n;
    }));
  }
  throw new Error('bytes input must use JSON array syntax like [0, 255, 10]');
}

export function isDomImportName(name) {
  return name.startsWith('dom_') || /^[A-Z][A-Za-z0-9]*\.[a-z][A-Za-z0-9_]*(?:\/[A-Za-z0-9_]+)?$/.test(name);
}

export function getWasmImports(wasmModule) {
  if (!wasmModule) return [];
  return WebAssembly.Module.imports(wasmModule);
}

export function usesDomImports(wasmModule) {
  return getWasmImports(wasmModule).some((wasmImport) =>
    wasmImport.module === WALUAU_IMPORT_MODULE &&
    wasmImport.kind === 'function' &&
    isDomImportName(wasmImport.name)
  );
}

function parseDomInterfaceImport(name) {
  const match = /^([A-Z][A-Za-z0-9]*)\.([a-z][A-Za-z0-9_]*(?:\/[A-Za-z0-9_]+)?)$/.exec(name);
  if (!match) return null;
  return {
    interfaceName: match[1],
    memberName: match[2],
  };
}

function createPlaygroundDomHost(wasmModule, domOutputRoot, getWasmExports = () => null) {
  const fallbackStorage = new Map();
  const fallbackStorageHost = {
    getItem(key) {
      const normalized = String(key);
      return fallbackStorage.has(normalized) ? fallbackStorage.get(normalized) : null;
    },
    setItem(key, value) {
      fallbackStorage.set(String(key), String(value));
    },
    removeItem(key) {
      fallbackStorage.delete(String(key));
    },
  };

  const outputDocument = () => {
    if (!domOutputRoot) {
      throw new Error('DOM Output root is not mounted');
    }
    return domOutputRoot;
  };

  const playgroundStorage = () => {
    const document = outputDocument();
    try {
      const storage = document.defaultView?.localStorage;
      if (storage?.getItem && storage?.setItem && storage?.removeItem) {
        return storage;
      }
    } catch {
      // Fall back below for sandboxed documents where localStorage is unavailable.
    }
    return fallbackStorageHost;
  };

  const replaceChild = (parent, newChild, oldChild) => {
    const removed = parent.replaceChild(newChild, oldChild);
    cleanupDomEventListeners(removed);
    return removed;
  };

  const removeChild = (parent, child) => {
    const removed = parent.removeChild(child);
    cleanupDomEventListeners(removed);
    return removed;
  };

  const registerEventListener = (target, type, callback) => {
    const listener = (event) => {
      const exports = getWasmExports();
      const trampoline = exports?.__waluau_call_callback_event_unit;
      if (typeof trampoline !== 'function') {
        throw new Error('Missing __waluau_call_callback_event_unit export for DOM event callback');
      }
      trampoline(callback, event);
    };
    target.addEventListener(type, listener);
    rememberDomEventListener(target, type, listener);
  };

  const getProperty = (interfaceName, propertyName, receiver) => {
    if (interfaceName === 'Window' && propertyName === 'localStorage') {
      return playgroundStorage();
    }
    return receiver[propertyName];
  };

  const setProperty = (_interfaceName, propertyName, receiver, value) => {
    receiver[propertyName] = value;
  };

  const forwardMethod = (_interfaceName, methodName, receiver, args) => {
    return receiver[methodName](...args);
  };

  const specialImports = {
    dom_window: () => outputDocument().defaultView,
    'EventTarget.addEventListener': (target, type, callback) => registerEventListener(target, String(type), callback),
    'Node.removeChild': removeChild,
    'Node.replaceChild': replaceChild,
    'Window.get/localStorage': () => playgroundStorage(),
  };

  const domImports = {};
  for (const wasmImport of getWasmImports(wasmModule)) {
    if (
      wasmImport.module !== WALUAU_IMPORT_MODULE ||
      wasmImport.kind !== 'function' ||
      !isDomImportName(wasmImport.name)
    ) {
      continue;
    }
    if (Object.prototype.hasOwnProperty.call(specialImports, wasmImport.name)) {
      domImports[wasmImport.name] = specialImports[wasmImport.name];
      continue;
    }

    const parsed = parseDomInterfaceImport(wasmImport.name);
    if (!parsed) continue;

    const { interfaceName, memberName } = parsed;
    if (memberName.startsWith('get/')) {
      const propertyName = memberName.slice(4);
      domImports[wasmImport.name] = (receiver) =>
        getProperty(interfaceName, propertyName, receiver);
    } else if (memberName.startsWith('set/')) {
      const propertyName = memberName.slice(4);
      domImports[wasmImport.name] = (receiver, value) =>
        setProperty(interfaceName, propertyName, receiver, value);
    } else {
      const methodName = memberName;
      domImports[wasmImport.name] = (receiver, ...args) =>
        forwardMethod(interfaceName, methodName, receiver, args);
    }
  }

  return {
    ...specialImports,
    ...domImports,
  };
}

export function buildWaluauImports(wasmModule, initLogger, options = {}) {
  const bytesConstants = decodeBytesConstantsFromWasm(wasmModule);
  const domHost = createPlaygroundDomHost(wasmModule, options.domOutputRoot, options.getWasmExports);
  const hostImports = options.hostImports ?? {};
  const reportAsyncError = (error) => {
    if (typeof options.onAsyncError === 'function') {
      options.onAsyncError(error);
      return;
    }
    queueMicrotask(() => {
      throw error;
    });
  };
  const asBytes = (value) => {
    if (value instanceof Uint8Array) return value;
    throw new Error(`Expected Uint8Array bytes value, got ${Object.prototype.toString.call(value)}`);
  };
  const externIs = (value, typeName) => {
    const name = String(typeName);
    const view = value?.ownerDocument?.defaultView ?? (value?.nodeType === 9 ? value.defaultView : globalThis);
    const ctor = view?.[name] ?? globalThis[name];
    return typeof ctor === 'function' && value instanceof ctor ? 1 : 0;
  };
  const waluauImports = {
    ...domHost,
    ...hostImports,
    __waluau_attach_promise: (threadHandle, promise) => {
      const exports = options.getWasmExports?.();
      const resume = exports?.[PROMISE_RESUME_TRAMPOLINE_EXPORT];
      const resetActive = exports?.[PROMISE_RESET_ACTIVE_EXPORT];
      if (typeof resume !== 'function') {
        throw new Error(`Missing ${PROMISE_RESUME_TRAMPOLINE_EXPORT} export for Promise await`);
      }
      if (typeof resetActive !== 'function') {
        throw new Error(`Missing ${PROMISE_RESET_ACTIVE_EXPORT} export for Promise await`);
      }
      if (promise == null || typeof promise.then !== 'function') {
        throw new TypeError('coroutine.await_promise expects a Promise-like extern value');
      }
      const invoke = (payload, rejected) => {
        try {
          resume(threadHandle, payload, rejected);
        } catch (error) {
          reportAsyncError(error);
        } finally {
          resetActive();
        }
      };
      Promise.resolve(promise).then(
        (value) => invoke(value, 0),
        (reason) => invoke(reason, 1),
      );
    },
    print: (value) => {
      if (printCaptureCallback) {
        printCaptureCallback(String(value));
      } else if (initLogger) {
        initLogger(String(value));
      } else {
        console.log(value);
      }
    },
    js_log: (value) => {
      if (printCaptureCallback) {
        printCaptureCallback(String(value));
      } else if (initLogger) {
        initLogger(String(value));
      } else {
        console.log(value);
      }
    },
    string_find: (haystack, needle, init, plain) => {
      const hay = String(haystack);
      const needleStr = String(needle);
      let start = Number(init);
      if (start < 0) start = Math.max(0, hay.length + start);
      // Pattern matching is not supported; only plain substring search.
      void plain;
      return hay.indexOf(needleStr, start);
    },
    extern_is: externIs,
    bytes_literal: (index) => {
      const literal = bytesConstants[index];
      if (!literal) {
        throw new Error(`Unknown bytes literal index ${index}`);
      }
      return literal.slice();
    },
    bytes_get: (value, index) => {
      const bytes = asBytes(value);
      if (index < 0 || index >= bytes.length) {
        throw new Error(`bytes index out of bounds: ${index}`);
      }
      return bytes[index];
    },
    bytes_len: (value) => asBytes(value).length,
    bytes_concat: (left, right) => {
      const a = asBytes(left);
      const b = asBytes(right);
      const merged = new Uint8Array(a.length + b.length);
      merged.set(a, 0);
      merged.set(b, a.length);
      return merged;
    },
    bytes_eq: (left, right) => {
      const a = asBytes(left);
      const b = asBytes(right);
      if (a.length !== b.length) return 0;
      for (let i = 0; i < a.length; i++) {
        if (a[i] !== b[i]) return 0;
      }
      return 1;
    },
    bytes_compare: (left, right) => {
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
    },
  };
  for (const wasmImport of getWasmImports(wasmModule)) {
    if (
      wasmImport.module === WALUAU_IMPORT_MODULE &&
      wasmImport.kind === 'function' &&
      wasmImport.name.startsWith('js_tostring_')
    ) {
      waluauImports[wasmImport.name] = (value) => String(value);
    }
  }
  for (const wasmImport of getWasmImports(wasmModule)) {
    if (
      wasmImport.module === WALUAU_IMPORT_MODULE &&
      wasmImport.kind === 'function' &&
      !Object.prototype.hasOwnProperty.call(waluauImports, wasmImport.name)
    ) {
      waluauImports[wasmImport.name] = () => {
        throw new Error(`Unsupported waluau import: ${wasmImport.name}`);
      };
    }
  }
  return {
    [WALUAU_IMPORT_MODULE]: waluauImports,
  };
}

// Parse WebAssembly binary to extract exports and signatures
export function getWasmExports(buffer) {
  if (!buffer) return [];
  const bytes = new Uint8Array(buffer);
  let pos = 8; // Skip magic number and version
  
  // Helper to read LEB128 unsigned integer
  function readVaruint() {
    let result = 0;
    let shift = 0;
    while (true) {
      if (pos >= bytes.length) return result;
      const byte = bytes[pos++];
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) break;
      shift += 7;
    }
    return result;
  }

  function readValTypeCode() {
    const byte = bytes[pos++];
    if (byte === 0x63 || byte === 0x64) {
      pos += 1; // heap type (e.g. extern)
      return byte === 0x63 ? 0x6f : 0x64;
    }
    return byte;
  }

  function skipTypeDefinition(form) {
    if (form === 0x60) {
      const numParams = readVaruint();
      for (let p = 0; p < numParams; p++) {
        readValTypeCode();
      }
      const numReturns = readVaruint();
      for (let r = 0; r < numReturns; r++) {
        readValTypeCode();
      }
      return;
    }
    if (form === 0x5e) {
      readValTypeCode(); // storage type
      pos += 1; // mutable flag
      return;
    }
    if (form === 0x5f) {
      const numFields = readVaruint();
      for (let f = 0; f < numFields; f++) {
        readValTypeCode(); // storage type
        pos += 1; // mutable flag
      }
      return;
    }
    // Unknown composite type: bail out to end of section by caller.
    throw new Error(`unsupported wasm type form: 0x${form.toString(16)}`);
  }

  const types = [];
  const funcTypeIndices = [];
  const exports = [];
  let importFuncCount = WALUAU_HOST_IMPORT_COUNT;

  while (pos < bytes.length) {
    const sectionId = bytes[pos++];
    const sectionLength = readVaruint();
    const sectionEnd = pos + sectionLength;
    if (sectionEnd > bytes.length) break;

    if (sectionId === 2) { // Import section
      const numImports = readVaruint();
      importFuncCount = 0;
      for (let i = 0; i < numImports; i++) {
        const moduleLen = readVaruint();
        pos += moduleLen;
        const nameLen = readVaruint();
        pos += nameLen;
        const kind = bytes[pos++];
        if (kind === 0) {
          importFuncCount += 1;
          readVaruint(); // type index
        } else if (kind === 1) {
          pos += 2; // table type
        } else if (kind === 2) {
          pos += 2; // memory limits
        } else if (kind === 3) {
          pos += 2; // global type
        }
      }
    } else if (sectionId === 1) { // Type section
      const numTypes = readVaruint();
      for (let i = 0; i < numTypes; i++) {
        const form = bytes[pos++];
        if (form === 0x60) {
          const numParams = readVaruint();
          const params = [];
          for (let p = 0; p < numParams; p++) {
            params.push(readValTypeCode());
          }
          const numReturns = readVaruint();
          const returns = [];
          for (let r = 0; r < numReturns; r++) {
            returns.push(readValTypeCode());
          }
          types.push({ params, returns });
        } else {
          // Non-function type (array, struct, …).  Push null to keep the
          // types[] index aligned with the Wasm type section index, which
          // funcTypeIndices references directly.
          try {
            skipTypeDefinition(form);
          } catch {
            pos = sectionEnd;
            break;
          }
          types.push(null);
        }
      }
    } else if (sectionId === 3) { // Function section
      const numFuncs = readVaruint();
      for (let i = 0; i < numFuncs; i++) {
        funcTypeIndices.push(readVaruint());
      }
    } else if (sectionId === 7) { // Export section
      const numExports = readVaruint();
      for (let i = 0; i < numExports; i++) {
        const nameLen = readVaruint();
        const nameBytes = bytes.subarray(pos, pos + nameLen);
        pos += nameLen;
        const name = new TextDecoder().decode(nameBytes);
        const kind = bytes[pos++];
        const index = readVaruint();
        if (kind === 0) { // function export
          exports.push({ name, index });
        }
      }
    } else {
      pos = sectionEnd;
    }
  }

  return exports.map(exp => {
    const definedIndex = exp.index - importFuncCount;
    const typeIdx = funcTypeIndices[definedIndex];
    const signature = types[typeIdx] || { params: [], returns: [] };
    return {
      name: exp.name,
      params: signature.params.map(typeCode => {
        if (typeCode === 0x7f) return 'i32';
        if (typeCode === 0x7e) return 'i64';
        if (typeCode === 0x7d) return 'f32';
        if (typeCode === 0x7c) return 'f64';
        if (typeCode === 0x6f || typeCode === 0x64) return 'string';
        return 'unknown';
      }),
      returns: signature.returns.map(typeCode => {
        if (typeCode === 0x7f) return 'i32';
        if (typeCode === 0x7e) return 'i64';
        if (typeCode === 0x7d) return 'f32';
        if (typeCode === 0x7c) return 'f64';
        if (typeCode === 0x6f || typeCode === 0x64) return 'string';
        return 'unknown';
      })
    };
  });
}

export function parseStringInput(valStr) {
  const trimmed = valStr.trim();
  if (
    trimmed.length >= 2 &&
    ((trimmed.startsWith('"') && trimmed.endsWith('"')) ||
     (trimmed.startsWith("'") && trimmed.endsWith("'")))
  ) {
    const inner = trimmed.slice(1, -1);
    let result = '';
    let i = 0;
    while (i < inner.length) {
      if (inner[i] === '\\' && i + 1 < inner.length) {
        const nextChar = inner[i + 1];
        if (nextChar === 'n') result += '\n';
        else if (nextChar === 't') result += '\t';
        else if (nextChar === 'r') result += '\r';
        else if (nextChar === '\\') result += '\\';
        else if (nextChar === '"') result += '"';
        else if (nextChar === "'") result += "'";
        else result += '\\' + nextChar;
        i += 2;
      } else {
        result += inner[i];
        i += 1;
      }
    }
    return result;
  }
  return valStr;
}

export function getEntries(obj) {
  if (!obj) return [];
  if (obj instanceof Map || typeof obj.entries === 'function') {
    return Array.from(obj.entries());
  }
  return Object.entries(obj);
}

export function renderType(type) {
  if (!type) return 'unknown';
  switch (type.kind) {
    case 'I32': return 'i32';
    case 'I64': return 'i64';
    case 'F32': return 'f32';
    case 'F64': return 'f64';
    case 'Bool': return 'bool';
    case 'String': return 'string';
    case 'Bytes': return 'bytes';
    case 'Unit': return 'unit';
    case 'Thread': return 'thread';
    case 'Array': return `{${renderType(type.value.elementType)}}`;
    case 'Record': {
      const fields = type.value.fields;
      const inner = getEntries(fields)
        .map(([name, ty]) => `${name}: ${renderType(ty)}`)
        .join(', ');
      return `{ ${inner} }`;
    }
    case 'TaggedUnion': {
      const inner = type.value.variants
        .map(v => `${v.tag}(${renderType(v.payload)})`)
        .join(' | ');
      return inner;
    }
    default: return 'unknown';
  }
}

export function getDefaultParamValue(type) {
  if (typeof type === 'object' && type !== null) {
    if (type.kind === 'Record') {
      const obj = {};
      for (const [name, fieldTy] of getEntries(type.value.fields)) {
        obj[name] = getDefaultParamValue(fieldTy);
      }
      return obj;
    }
    if (type.kind === 'TaggedUnion') {
      const firstVariant = type.value.variants[0];
      return firstVariant ? {
        tag: firstVariant.tag,
        value: getDefaultParamValue(firstVariant.payload)
      } : null;
    }
    if (type.kind === 'Array') {
      return '[]';
    }
    if (type.kind === 'String') return '""';
    if (type.kind === 'Bytes') return '[]';
    return '0';
  }
  return type === 'string' ? '""' : '0';
}

export function constructArg(val, type, instance, tagIds) {
  if (!type) return Number(val);
  const plainTagIds = (tagIds && typeof tagIds.entries === 'function')
    ? Object.fromEntries(tagIds.entries())
    : (tagIds || {});

  switch (type.kind) {
    case 'I32': {
      const n = Number(val);
      if (isNaN(n) || !Number.isInteger(n)) {
        throw new Error('must be a valid 32-bit integer');
      }
      return n;
    }
    case 'I64': {
      try {
        return BigInt(String(val).trim().replace(/n$/, ''));
      } catch {
        throw new Error('must be a valid 64-bit integer');
      }
    }
    case 'F32':
    case 'F64': {
      const n = Number(val);
      if (isNaN(n)) {
        throw new Error('must be a valid number');
      }
      return n;
    }
    case 'Bool': {
      return (val === 'true' || val === true || val === '1' || Number(val) === 1) ? 1 : 0;
    }
    case 'String': {
      return parseStringInput(String(val));
    }
    case 'Bytes': {
      return parseBytesInput(String(val));
    }
    case 'Unit': {
      return null;
    }
    case 'Record': {
      const typeIdx = type.value.typeIndex;
      const ctorName = `__waluau_new_record_${typeIdx}`;
      const ctor = instance.exports[ctorName];
      if (!ctor) {
        throw new Error(`Constructor ${ctorName} not found`);
      }
      const args = getEntries(type.value.fields).map(([name, fieldTy]) => {
        const fieldVal = val ? val[name] : null;
        return constructArg(fieldVal, fieldTy, instance, plainTagIds);
      });
      return ctor(...args);
    }
    case 'TaggedUnion': {
      const typeIdx = type.value.typeIndex;
      const ctorName = `__waluau_new_record_${typeIdx}`;
      const ctor = instance.exports[ctorName];
      if (!ctor) {
        throw new Error(`Constructor ${ctorName} not found`);
      }
      const selectedTag = val?.tag;
      const variant = type.value.variants.find(v => v.tag === selectedTag);
      if (!variant) {
        throw new Error(`Variant "${selectedTag}" not found in union`);
      }
      const tagId = plainTagIds[selectedTag];
      if (tagId === undefined) {
        throw new Error(`Tag ID for variant "${selectedTag}" not found`);
      }
      const payloadVal = val ? val.value : null;
      const constructedPayload = constructArg(payloadVal, variant.payload, instance, plainTagIds);
      return ctor(tagId, constructedPayload);
    }
    default: {
      return Number(val);
    }
  }
}

export function inspectVal(val, type, instance, tagIds) {
  if (val === null || val === undefined) return null;
  if (!type) return val;
  const plainTagIds = (tagIds && typeof tagIds.entries === 'function')
    ? Object.fromEntries(tagIds.entries())
    : (tagIds || {});

  switch (type.kind) {
    case 'I32': return Number(val);
    case 'I64': return { _isBigInt: true, val: BigInt(val) };
    case 'F32':
    case 'F64': return Number(val);
    case 'Bool': return Boolean(val);
    case 'String': return String(val);
    case 'Bytes': return { _isBytes: true, bytes: Array.from(val instanceof Uint8Array ? val : []) };
    case 'Unit': return null;
    case 'Record': {
      const typeIdx = type.value.typeIndex;
      const obj = {};
      getEntries(type.value.fields).forEach(([fieldName, fieldTy], fieldIdx) => {
        const getterName = `__waluau_get_record_${typeIdx}_${fieldIdx}`;
        const getter = instance.exports[getterName];
        if (getter) {
          const fieldVal = getter(val);
          obj[fieldName] = inspectVal(fieldVal, fieldTy, instance, plainTagIds);
        } else {
          obj[fieldName] = 'undefined';
        }
      });
      return obj;
    }
    case 'TaggedUnion': {
      const typeIdx = type.value.typeIndex;
      const tagGetterName = `__waluau_get_record_${typeIdx}_0`;
      const valGetterName = `__waluau_get_record_${typeIdx}_1`;
      const tagGetter = instance.exports[tagGetterName];
      const valGetter = instance.exports[valGetterName];
      if (!tagGetter || !valGetter) {
        return { _isTaggedUnion: true, tag: 'unknown', value: 'undefined' };
      }
      const tagVal = tagGetter(val);
      const payloadVal = valGetter(val);
      
      const tagNames = Object.fromEntries(Object.entries(plainTagIds).map(([k, v]) => [v, k]));
      const tagName = tagNames[tagVal] ?? `UnknownTag(${tagVal})`;
      
      const variant = type.value.variants.find(v => v.tag === tagName);
      const payloadTy = variant ? variant.payload : null;
      
      return {
        _isTaggedUnion: true,
        tag: tagName,
        value: inspectVal(payloadVal, payloadTy, instance, plainTagIds),
      };
    }
    default: return val;
  }
}

export function formatInspectedVal(inspectedVal) {
  if (inspectedVal === null || inspectedVal === undefined) return 'nil';
  if (typeof inspectedVal === 'object') {
    if (inspectedVal._isBigInt) {
      return inspectedVal.val.toString() + 'n';
    }
    if (inspectedVal._isBytes) {
      return `bytes[${inspectedVal.bytes.join(', ')}]`;
    }
    if (inspectedVal._isTaggedUnion) {
      if (inspectedVal.value === null || inspectedVal.value === undefined) {
        return `${inspectedVal.tag}()`;
      }
      return `${inspectedVal.tag}(${formatInspectedVal(inspectedVal.value)})`;
    }
    if (Array.isArray(inspectedVal)) {
      return '{' + inspectedVal.map(formatInspectedVal).join(', ') + '}';
    }
    const inner = Object.entries(inspectedVal)
      .map(([name, val]) => `${name} = ${formatInspectedVal(val)}`)
      .join(', ');
    return `{ ${inner} }`;
  }
  if (typeof inspectedVal === 'string') {
    return JSON.stringify(inspectedVal);
  }
  return String(inspectedVal);
}

export function executeCall(instance, funcName, paramsInfo, richParamsInfo, richReturnsInfo, inputValues, tagIds) {
  if (!instance) return { error: 'No instance' };
  const func = instance.exports[funcName];
  if (!func) return { error: `Exported function "${funcName}" not found` };

  const logs = [];
  printCaptureCallback = (msg) => {
    logs.push(msg);
  };

  try {
    const parsedArgs = [];
    for (let i = 0; i < paramsInfo.length; i++) {
      const type = paramsInfo[i];
      const richType = richParamsInfo ? richParamsInfo[i] : null;
      const val = inputValues[i];

      if (richType) {
        try {
          parsedArgs.push(constructArg(val, richType, instance, tagIds));
        } catch (err) {
          return { error: `Parameter ${i} error: ${err.message}`, logs };
        }
      } else {
        const valStr = val || '0';
        if (type === 'i64') {
          try {
            parsedArgs.push(BigInt(valStr.trim().replace(/n$/, '')));
          } catch {
            return { error: `Parameter ${i} must be a valid 64-bit integer`, logs };
          }
        } else if (type === 'i32') {
          const num = Number(valStr);
          if (isNaN(num) || !Number.isInteger(num)) {
            return { error: `Parameter ${i} must be a valid 32-bit integer`, logs };
          }
          parsedArgs.push(num);
        } else if (type === 'f32' || type === 'f64') {
          const num = Number(valStr);
          if (isNaN(num)) {
            return { error: `Parameter ${i} must be a valid number`, logs };
          }
          parsedArgs.push(num);
        } else if (type === 'string') {
          parsedArgs.push(parseStringInput(valStr));
        } else {
          parsedArgs.push(Number(valStr));
        }
      }
    }

    const result = func(...parsedArgs);
    let valStr = '';
    if (richReturnsInfo && richReturnsInfo.length > 0) {
      if (richReturnsInfo.length === 1) {
        const inspected = inspectVal(result, richReturnsInfo[0], instance, tagIds);
        valStr = formatInspectedVal(inspected);
      } else {
        const inspected = richReturnsInfo.map((retTy, rIdx) => {
          const retVal = Array.isArray(result) ? result[rIdx] : (rIdx === 0 ? result : null);
          return inspectVal(retVal, retTy, instance, tagIds);
        });
        valStr = inspected.map(formatInspectedVal).join(', ');
      }
    } else {
      if (typeof result === 'bigint') {
        valStr = result.toString() + 'n';
      } else if (typeof result === 'string') {
        valStr = JSON.stringify(result);
      } else {
        valStr = String(result);
      }
    }
    return { value: valStr, logs };
  } catch (err) {
    return { error: `Execution crashed: ${err.message}`, logs };
  } finally {
    printCaptureCallback = null;
  }
}

export function classifyWasmInstantiationError(err, requiresWasmGc) {
  const message = err?.message || String(err);
  const isCompileError =
    (typeof WebAssembly !== 'undefined' &&
      (err instanceof WebAssembly.CompileError ||
       err instanceof WebAssembly.LinkError)) ||
    err?.name === 'CompileError' ||
    err?.name === 'LinkError';

  if (requiresWasmGc && isCompileError) {
    return [
      'This module requires Wasm GC (array reference types), which may not be supported or enabled in this browser.',
      `Instantiation error: ${message}`
    ].join('\n');
  }
  return `Failed to instantiate WASM module: ${message}`;
}
