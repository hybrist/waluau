export const WALUAU_STRING_CONSTANTS_MODULE = 'string_constants';
export const WALUAU_IMPORT_MODULE = 'waluau';
// Must match waluau_codegen_wasm::host::HOST_IMPORT_COUNT
export const WALUAU_HOST_IMPORT_COUNT = 16;

const DOM_IMPORT_NAMES = new Set([
  'dom_append_child',
  'dom_clear',
  'dom_create_element',
  'dom_document',
  'dom_set_text',
  'Document.append_child',
  'Document.create_element',
  'Document.get_element_by_id',
  'Element.append',
  'Element.append_child',
  'Element.clear',
  'Element.get_inner_text',
  'Element.set_attr',
  'Element.set_class',
  'Element.set_inner_text',
  'Element.set_text',
  'Node.append_child',
]);

const BLOCKED_DOM_TAGS = new Set([
  'base',
  'embed',
  'iframe',
  'link',
  'meta',
  'object',
  'script',
  'style',
]);

let printCaptureCallback = null;

export function decodeBytesConstantsFromWasm(wasmBuffer) {
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
    if (sectionEnd > bytes.length) break;

    if (sectionId === 0) {
      const nameLen = readVaruint();
      const nameStart = pos;
      const nameEnd = nameStart + nameLen;
      const nameBytes = bytes.subarray(nameStart, nameEnd);
      const name = new TextDecoder().decode(nameBytes);
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
  return name.startsWith('dom_') || DOM_IMPORT_NAMES.has(name);
}

export function getWasmImports(buffer) {
  if (!buffer) return [];
  const bytes = new Uint8Array(buffer);
  let pos = 8;
  const imports = [];

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

  function readName() {
    const len = readVaruint();
    const value = new TextDecoder().decode(bytes.subarray(pos, pos + len));
    pos += len;
    return value;
  }

  function skipLimits() {
    const flags = readVaruint();
    readVaruint();
    if (flags & 0x01) readVaruint();
  }

  function skipValType() {
    const byte = bytes[pos++];
    if (byte === 0x63 || byte === 0x64) {
      pos += 1;
    }
  }

  while (pos < bytes.length) {
    const sectionId = bytes[pos++];
    const sectionLength = readVaruint();
    const sectionEnd = pos + sectionLength;
    if (sectionEnd > bytes.length) break;

    if (sectionId !== 2) {
      pos = sectionEnd;
      continue;
    }

    const numImports = readVaruint();
    for (let i = 0; i < numImports; i++) {
      const module = readName();
      const name = readName();
      const kind = bytes[pos++];
      if (kind === 0) {
        const typeIndex = readVaruint();
        imports.push({ module, name, kind: 'function', typeIndex });
      } else if (kind === 1) {
        skipValType();
        skipLimits();
        imports.push({ module, name, kind: 'table' });
      } else if (kind === 2) {
        skipLimits();
        imports.push({ module, name, kind: 'memory' });
      } else if (kind === 3) {
        skipValType();
        pos += 1;
        imports.push({ module, name, kind: 'global' });
      } else {
        pos = sectionEnd;
        break;
      }
    }
    pos = sectionEnd;
  }

  return imports;
}

export function usesDomImports(wasmBuffer) {
  return getWasmImports(wasmBuffer).some((wasmImport) =>
    wasmImport.module === WALUAU_IMPORT_MODULE &&
    wasmImport.kind === 'function' &&
    isDomImportName(wasmImport.name)
  );
}

function createPlaygroundDomHost(domOutputRoot) {
  const requireOutputDocument = () => {
    if (!domOutputRoot) {
      throw new Error('DOM Output root is not mounted');
    }
    if (domOutputRoot.nodeType !== Node.DOCUMENT_NODE || typeof domOutputRoot.createElement !== 'function') {
      throw new Error('DOM Output root must be a Document');
    }
    return domOutputRoot;
  };

  const requireElement = (value, label = 'DOM host value') => {
    const ElementCtor = value?.ownerDocument?.defaultView?.Element ?? Element;
    if (!(value instanceof ElementCtor)) {
      throw new Error(`${label} must be an Element`);
    }
    return value;
  };

  const requireAppendTarget = (value, label = 'DOM parent') => {
    const document = requireOutputDocument();
    if (value === document) {
      return document.body;
    }
    return requireElement(value, label);
  };

  const clearTarget = (value) => {
    const document = requireOutputDocument();
    if (value === document) {
      return document.body;
    }
    return requireElement(value);
  };

  const normalizeTag = (tag) => {
    const normalized = String(tag).trim().toLowerCase();
    if (!/^[a-z][a-z0-9-]*$/.test(normalized) || BLOCKED_DOM_TAGS.has(normalized)) {
      throw new Error(`Unsupported DOM tag: ${tag}`);
    }
    return normalized;
  };

  const createElement = (_document, tag) => {
    return requireOutputDocument().createElement(normalizeTag(tag));
  };

  const appendChild = (parent, child) => {
    requireAppendTarget(parent).appendChild(requireElement(child, 'DOM child'));
  };

  const clear = (element) => {
    clearTarget(element).replaceChildren();
  };

  const setText = (element, text) => {
    requireElement(element).textContent = String(text);
  };

  const getInnerText = (element) => {
    return requireElement(element).textContent;
  };

  const setClass = (element, className) => {
    requireElement(element).className = String(className);
  };

  const setAttr = (element, name, value) => {
    const attrName = String(name).trim().toLowerCase();
    if (!/^[a-z_:][a-z0-9_:.-]*$/.test(attrName) || attrName.startsWith('on')) {
      throw new Error(`Unsupported DOM attribute: ${name}`);
    }
    requireElement(element).setAttribute(attrName, String(value));
  };

  return {
    dom_document: () => requireOutputDocument(),
    dom_root: () => requireOutputDocument().body,
    dom_output_root: () => requireOutputDocument().body,
    dom_create_element: createElement,
    dom_append_child: appendChild,
    dom_clear: clear,
    dom_set_text: setText,
    'Document.append_child': appendChild,
    'Document.create_element': createElement,
    'Element.append': appendChild,
    'Element.append_child': appendChild,
    'Element.clear': clear,
    'Element.get_inner_text': getInnerText,
    'Element.set_inner_text': setText,
    'Element.set_text': setText,
    'Element.set_class': setClass,
    'Element.set_attr': setAttr,
    'Node.append_child': appendChild,
  };
}

export function buildWaluauImports(wasmBuffer, initLogger, options = {}) {
  const bytesConstants = decodeBytesConstantsFromWasm(wasmBuffer);
  const domHost = createPlaygroundDomHost(options.domOutputRoot);
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
        return (value) => {
          if (printCaptureCallback) {
            printCaptureCallback(String(value));
          } else if (initLogger) {
            initLogger(String(value));
          } else {
            console.log(value);
          }
        };
      }
      if (Object.prototype.hasOwnProperty.call(domHost, name)) {
        return domHost[name];
      }
      if (name === 'bytes_literal') {
        return (index) => {
          const literal = bytesConstants[index];
          if (!literal) {
            throw new Error(`Unknown bytes literal index ${index}`);
          }
          return literal.slice();
        };
      }
      if (name === 'bytes_get') {
        return (value, index) => {
          const bytes = asBytes(value);
          if (index < 0 || index >= bytes.length) {
            throw new Error(`bytes index out of bounds: ${index}`);
          }
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
    if (type.kind === 'Array') {
      return '[]';
    }
    if (type.kind === 'String') return '""';
    if (type.kind === 'Bytes') return '[]';
    return '0';
  }
  return type === 'string' ? '""' : '0';
}

export function constructArg(val, type, instance) {
  if (!type) return Number(val);
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
    case 'Record': {
      const typeIdx = type.value.typeIndex;
      const ctorName = `__waluau_new_record_${typeIdx}`;
      const ctor = instance.exports[ctorName];
      if (!ctor) {
        throw new Error(`Constructor ${ctorName} not found`);
      }
      const args = getEntries(type.value.fields).map(([name, fieldTy]) => {
        const fieldVal = val ? val[name] : null;
        return constructArg(fieldVal, fieldTy, instance);
      });
      return ctor(...args);
    }
    default: {
      return Number(val);
    }
  }
}

export function inspectVal(val, type, instance) {
  if (val === null || val === undefined) return null;
  if (!type) return val;
  switch (type.kind) {
    case 'I32': return Number(val);
    case 'I64': return { _isBigInt: true, val: BigInt(val) };
    case 'F32':
    case 'F64': return Number(val);
    case 'Bool': return Boolean(val);
    case 'String': return String(val);
    case 'Bytes': return { _isBytes: true, bytes: Array.from(val instanceof Uint8Array ? val : []) };
    case 'Record': {
      const typeIdx = type.value.typeIndex;
      const obj = {};
      getEntries(type.value.fields).forEach(([fieldName, fieldTy], fieldIdx) => {
        const getterName = `__waluau_get_record_${typeIdx}_${fieldIdx}`;
        const getter = instance.exports[getterName];
        if (getter) {
          const fieldVal = getter(val);
          obj[fieldName] = inspectVal(fieldVal, fieldTy, instance);
        } else {
          obj[fieldName] = 'undefined';
        }
      });
      return obj;
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

export function executeCall(instance, funcName, paramsInfo, richParamsInfo, richReturnsInfo, inputValues) {
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
          parsedArgs.push(constructArg(val, richType, instance));
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
        const inspected = inspectVal(result, richReturnsInfo[0], instance);
        valStr = formatInspectedVal(inspected);
      } else {
        const inspected = richReturnsInfo.map((retTy, rIdx) => {
          const retVal = Array.isArray(result) ? result[rIdx] : (rIdx === 0 ? result : null);
          return inspectVal(retVal, retTy, instance);
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
