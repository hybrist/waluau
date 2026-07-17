import * as tf from '@tensorflow/tfjs';
import { luaPatternMatch, luaGsub, luaGsubGenerator, luaGmatch, makeStringReplacer } from './lua-pattern.js';

export const WALUAU_STRING_CONSTANTS_MODULE = 'string_constants';
export const WALUAU_IMPORT_MODULE = 'waluau';
export const WALUAU_MAIN_EXPORT = '__waluau_main';
// Must match waluau_codegen_wasm::host::HOST_IMPORT_COUNT
export const WALUAU_HOST_IMPORT_COUNT = 24;
const PROMISE_RESUME_TRAMPOLINE_EXPORT = '__waluau_resume_promise_await';
const PROMISE_RESET_ACTIVE_EXPORT = '__waluau_reset_active_coroutine';
const CALLBACK_UNIT_EXTERN_TRAMPOLINE_EXPORT = '__waluau_call_callback_unit_extern';
const CALLBACK_F64_UNIT_TRAMPOLINE_EXPORT = '__waluau_call_callback_f64_unit';

let printCaptureCallback = null;
const domEventListeners = new WeakMap();
// Window -> Set of pending requestAnimationFrame handles scheduled by wasm
// callbacks. Cancelled on rerun so a superseded instance's frame loop stops.
const pendingAnimationFrames = new WeakMap();

// Luau formats numbers with `%.14g`: `nan`/`inf`/`-inf` for specials, `-0`
// preserved, and at most 14 significant digits. JS `String()` disagrees on all
// of those (`NaN`, `Infinity`, `0`, full precision), so `tostring` host
// imports route numbers through this instead.
export function luauToString(value) {
  if (typeof value !== 'number') return String(value);
  if (Number.isNaN(value)) return 'nan';
  if (value === Infinity) return 'inf';
  if (value === -Infinity) return '-inf';
  if (Object.is(value, -0)) return '-0';
  return String(Number(value.toPrecision(14)));
}

function luauTypeName(value) {
  if (value == null) return 'nil';
  switch (typeof value) {
    case 'boolean': return 'boolean';
    case 'number':
    case 'bigint':
      return 'number';
    case 'string': return 'string';
    case 'function': return 'function';
    case 'object': return 'table';
    default: return 'userdata';
  }
}

function luauToNumber(value, base = 0) {
  const radix = Number(base) | 0;
  if (radix !== 0) {
    if (typeof value !== 'string' || radix < 2 || radix > 36) return NaN;
    const text = value.trim();
    if (text === '') return NaN;
    const sign = text.startsWith('-') ? -1 : 1;
    const digits = text.replace(/^[+-]/, '');
    const valid = new RegExp(`^[0-${Math.min(radix - 1, 9)}${radix > 10 ? `a-${String.fromCharCode(86 + radix)}` : ''}]+$`, 'i');
    if (!valid.test(digits)) return NaN;
    return sign * parseInt(digits, radix);
  }
  if (typeof value === 'number') return value;
  if (typeof value !== 'string') return NaN;
  const text = value.trim();
  if (text === '') return NaN;
  const parsed = Number(text);
  return Number.isNaN(parsed) ? NaN : parsed;
}

function luaQuoteString(value) {
  return JSON.stringify(String(value))
    .replace(/\u2028/g, '\\u2028')
    .replace(/\u2029/g, '\\u2029');
}

function formatNumber(value, specifier, precision) {
  const number = Number(value);
  switch (specifier) {
    case 'd':
    case 'i':
      return String(Math.trunc(number));
    case 'u':
      return BigInt.asUintN(64, BigInt(Math.trunc(number))).toString(10);
    case 'o':
      return BigInt.asUintN(64, BigInt(Math.trunc(number))).toString(8);
    case 'x':
      return BigInt.asUintN(64, BigInt(Math.trunc(number))).toString(16);
    case 'X':
      return BigInt.asUintN(64, BigInt(Math.trunc(number))).toString(16).toUpperCase();
    case 'f':
      return number.toFixed(precision ?? 6);
    case 'g':
    case 'G':
      return precision == null ? luauToString(number) : String(Number(number.toPrecision(precision)));
    default:
      return luauToString(number);
  }
}

function padFormatted(value, flags, width, specifier) {
  const minWidth = width == null ? 0 : width;
  if (value.length >= minWidth) return value;
  const zeroPad = flags.includes('0') && !flags.includes('-') && !['s', 'q', 'c'].includes(specifier);
  const padding = (zeroPad ? '0' : ' ').repeat(minWidth - value.length);
  return flags.includes('-') ? value + padding : padding + value;
}

function stringFormat(format, ...args) {
  const fmt = String(format);
  let argIndex = 0;
  let out = '';
  for (let i = 0; i < fmt.length; i++) {
    const ch = fmt[i];
    if (ch !== '%') {
      out += ch;
      continue;
    }
    if (i + 1 >= fmt.length) {
      throw new Error('incomplete string.format specifier');
    }
    let flags = '';
    while (i + 1 < fmt.length && '-+ #0'.includes(fmt[i + 1])) {
      flags += fmt[++i];
      if (flags.length > 16) throw new Error('invalid string.format flags');
    }
    let widthText = '';
    while (i + 1 < fmt.length && /[0-9]/.test(fmt[i + 1])) widthText += fmt[++i];
    let precision = null;
    if (i + 1 < fmt.length && fmt[i + 1] === '.') {
      i++;
      let precisionText = '';
      while (i + 1 < fmt.length && /[0-9]/.test(fmt[i + 1])) precisionText += fmt[++i];
      if (precisionText === '' || precisionText.length > 2) {
        throw new Error('invalid string.format precision');
      }
      precision = Number(precisionText);
    }
    if (widthText.length > 2) throw new Error('invalid string.format width');
    const width = widthText === '' ? null : Number(widthText);
    const specifier = fmt[++i];
    if (specifier === '%') {
      out += '%';
      continue;
    }
    if (specifier === '*') {
      if (argIndex >= args.length) throw new Error('not enough arguments for string.format');
      out += String(args[argIndex++]);
      continue;
    }
    if (!'diuofegGxXqsc'.includes(specifier)) {
      throw new Error(`unsupported string.format specifier %${specifier}`);
    }
    if (argIndex >= args.length) {
      throw new Error('not enough arguments for string.format');
    }
    const value = args[argIndex++];
    let formatted;
    if (specifier === 's') {
      formatted = String(value);
      if (precision != null) formatted = formatted.slice(0, precision);
    } else if (specifier === 'q') {
      formatted = luaQuoteString(value);
    } else if (specifier === 'c') {
      formatted = String.fromCodePoint(Number(value) & 0xff);
    } else {
      formatted = formatNumber(value, specifier, precision);
    }
    out += padFormatted(formatted, flags, width, specifier);
  }
  return out;
}

function stringChar(...args) {
  const codes = args.map((value) => Number(value));
  for (const code of codes) {
    if (!Number.isInteger(code) || code < 0 || code > 255) {
      throw new Error(`string.char code out of range: ${code}`);
    }
  }
  return String.fromCodePoint(...codes);
}

// Host side of the Lua pattern-matching builtins (string.find/match/gmatch/
// gsub). The compiler lowers those builtins to the pm_* imports below; the
// last successful match's bounds and captures are read back through the
// pm_match_*/pm_capture_* accessors immediately after the call that produced
// them. gsub-with-function-replacement and gmatch iterate via integer handles
// so nested iterations do not clobber each other.
function createLuaPatternHost() {
  let lastMatch = null; // { start, end (1-based inclusive), whole, captures }
  let lastGsubCount = 0;
  const handles = new Map();
  let nextHandle = 1;

  // Lua's posrelat + str_find_aux init clamping (1-based; negative counts
  // from the end; anything past len+1 can never match).
  const normalizeInit = (init, len) => {
    let pos = Number(init) | 0;
    if (pos < 0) pos = -pos > len ? 0 : len + pos + 1;
    if (pos < 1) pos = 1;
    return pos > len + 1 ? null : pos;
  };

  const rememberMatch = (source, m) => {
    lastMatch = {
      start: m.start + 1,
      end: m.end,
      whole: source.slice(m.start, m.end),
      captures: m.captures,
    };
  };

  return {
    pm_find: (haystack, pattern, init, plain) => {
      const str = String(haystack);
      const pat = String(pattern);
      const start = normalizeInit(init, str.length);
      if (start === null) {
        lastMatch = null;
        return 0;
      }
      if (plain) {
        const index = str.indexOf(pat, start - 1);
        if (index < 0) {
          lastMatch = null;
          return 0;
        }
        lastMatch = {
          start: index + 1,
          end: index + pat.length,
          whole: pat,
          captures: [],
        };
        return 1;
      }
      const m = luaPatternMatch(str, pat, start - 1, false);
      if (!m) {
        lastMatch = null;
        return 0;
      }
      rememberMatch(str, m);
      return 1;
    },
    pm_match: (haystack, pattern, init) => {
      const str = String(haystack);
      const start = normalizeInit(init, str.length);
      if (start === null) {
        lastMatch = null;
        return 0;
      }
      const m = luaPatternMatch(str, String(pattern), start - 1, true);
      if (!m) {
        lastMatch = null;
        return 0;
      }
      rememberMatch(str, m);
      return 1;
    },
    pm_match_start: () => (lastMatch ? lastMatch.start : 0),
    pm_match_end: () => (lastMatch ? lastMatch.end : 0),
    pm_capture_string: (index) => {
      if (!lastMatch) return '';
      const i = Number(index);
      if (i === 0) return lastMatch.whole;
      const capture = lastMatch.captures[i - 1];
      return capture === undefined ? '' : String(capture.value);
    },
    pm_capture_position: (index) => {
      if (!lastMatch) return 0;
      const capture = lastMatch.captures[Number(index) - 1];
      return capture === undefined ? 0 : Number(capture.value);
    },
    pm_gsub: (source, pattern, replacement, maxCount) => {
      const { result, count } = luaGsub(
        String(source),
        String(pattern),
        makeStringReplacer(String(replacement)),
        Number(maxCount),
      );
      lastGsubCount = count;
      return result;
    },
    pm_gsub_count: () => lastGsubCount,
    pm_gsub_begin: (source, pattern, maxCount) => {
      const handle = nextHandle++;
      handles.set(handle, {
        gen: luaGsubGenerator(String(source), String(pattern), Number(maxCount)),
        replacement: undefined,
        result: null,
      });
      return handle;
    },
    pm_gsub_next: (handle) => {
      const state = handles.get(Number(handle));
      const step = state.gen.next(state.replacement);
      state.replacement = undefined;
      if (step.done) {
        state.result = step.value.result;
        lastGsubCount = step.value.count;
        return 0;
      }
      lastMatch = {
        start: 0,
        end: 0,
        whole: step.value.whole,
        captures: step.value.captures,
      };
      return 1;
    },
    pm_gsub_replace: (handle, replacement) => {
      handles.get(Number(handle)).replacement = String(replacement);
    },
    pm_gsub_keep: (handle) => {
      handles.get(Number(handle)).replacement = null;
    },
    pm_gsub_finish: (handle) => {
      const state = handles.get(Number(handle));
      handles.delete(Number(handle));
      return state.result ?? '';
    },
    pm_gmatch: (haystack, pattern) => {
      const source = String(haystack);
      const handle = nextHandle++;
      handles.set(handle, { next: luaGmatch(source, String(pattern)), source });
      return handle;
    },
    pm_gmatch_next: (handle) => {
      const state = handles.get(Number(handle));
      if (!state) return 0;
      const m = state.next();
      if (!m) {
        handles.delete(Number(handle));
        return 0;
      }
      rememberMatch(state.source, m);
      return 1;
    },
  };
}

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

export function cancelPendingAnimationFrames(documentOrWindow) {
  const view = documentOrWindow?.defaultView ?? documentOrWindow;
  const handles = view ? pendingAnimationFrames.get(view) : undefined;
  if (!handles) return;
  for (const handle of handles) {
    view.cancelAnimationFrame(handle);
  }
  handles.clear();
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

export function decodeWasmImports(wasmBytes) {
  const bytes = wasmBytes instanceof Uint8Array ? wasmBytes : new Uint8Array(wasmBytes);
  if (
    bytes.length < 8 ||
    bytes[0] !== 0x00 ||
    bytes[1] !== 0x61 ||
    bytes[2] !== 0x73 ||
    bytes[3] !== 0x6d
  ) {
    throw new Error('invalid WebAssembly module header while decoding imports');
  }

  let pos = 8;
  const decoder = new TextDecoder();
  const readVaruint = () => {
    let value = 0;
    let shift = 0;
    while (pos < bytes.length) {
      const byte = bytes[pos++];
      value |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) return value >>> 0;
      shift += 7;
      if (shift > 35) break;
    }
    throw new Error('invalid WebAssembly varuint while decoding imports');
  };
  const skipSignedLeb = () => {
    while (pos < bytes.length) {
      if ((bytes[pos++] & 0x80) === 0) return;
    }
    throw new Error('invalid WebAssembly heap type while decoding imports');
  };
  const skipUnsignedLeb = (maxBytes) => {
    for (let index = 0; index < maxBytes && pos < bytes.length; index += 1) {
      if ((bytes[pos++] & 0x80) === 0) return;
    }
    throw new Error('invalid WebAssembly limit while decoding imports');
  };
  const readName = () => {
    const length = readVaruint();
    const end = pos + length;
    if (end > bytes.length) throw new Error('truncated WebAssembly import name');
    const name = decoder.decode(bytes.subarray(pos, end));
    pos = end;
    return name;
  };
  const skipValueType = () => {
    const type = bytes[pos++];
    if (type === 0x63 || type === 0x64) skipSignedLeb();
  };
  const skipLimits = () => {
    const flags = readVaruint();
    const skipLimit = (flags & 0x04) !== 0
      ? () => skipUnsignedLeb(10)
      : () => { readVaruint(); };
    skipLimit();
    if ((flags & 0x01) !== 0) skipLimit();
    if ((flags & 0x08) !== 0) readVaruint();
  };

  while (pos < bytes.length) {
    const sectionId = bytes[pos++];
    const sectionLength = readVaruint();
    const sectionEnd = pos + sectionLength;
    if (sectionEnd > bytes.length) throw new Error('truncated WebAssembly section');
    if (sectionId !== 2) {
      pos = sectionEnd;
      continue;
    }

    const imports = [];
    const count = readVaruint();
    const kindNames = ['function', 'table', 'memory', 'global', 'tag'];
    for (let index = 0; index < count; index += 1) {
      const module = readName();
      const name = readName();
      const kindCode = bytes[pos++];
      const kind = kindNames[kindCode];
      if (!kind) throw new Error(`unsupported WebAssembly import kind ${kindCode}`);

      if (kindCode === 0) {
        readVaruint();
      } else if (kindCode === 1) {
        skipValueType();
        skipLimits();
      } else if (kindCode === 2) {
        skipLimits();
      } else if (kindCode === 3) {
        skipValueType();
        pos += 1;
      } else {
        readVaruint();
        readVaruint();
      }
      imports.push({ module, name, kind });
    }
    return imports;
  }
  return [];
}

export function getWasmImports(wasmModule, wasmBytes) {
  if (wasmBytes) return decodeWasmImports(wasmBytes);
  if (!wasmModule) return [];
  return WebAssembly.Module.imports(wasmModule);
}

export function usesDomImports(wasmModule, wasmBytes, requiredImports) {
  const imports = requiredImports ?? getWasmImports(wasmModule, wasmBytes);
  return imports.some((wasmImport) =>
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

function snakeToCamel(name) {
  return name.replace(/_([a-z])/g, (_match, letter) => letter.toUpperCase());
}

function resolveDomMemberName(receiver, generatedName) {
  if (receiver == null) {
    throw new TypeError(`DOM import receiver is null while resolving ${generatedName}`);
  }
  if (generatedName in receiver) {
    return generatedName;
  }
  const nativeName = snakeToCamel(generatedName);
  if (nativeName in receiver) {
    return nativeName;
  }
  return generatedName;
}

function createPlaygroundDomHost(wasmImports, domOutputRoot, getWasmExports = () => null, getWasmMemory = () => null) {
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

  const fallbackWindowHost = {
    get document() {
      return outputDocument();
    },
    get localStorage() {
      return playgroundStorage();
    },
  };

  const outputWindow = () => outputDocument().defaultView ?? fallbackWindowHost;

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

  const requestAnimationFrame = (target, callback) => {
    let handles = pendingAnimationFrames.get(target);
    if (!handles) {
      handles = new Set();
      pendingAnimationFrames.set(target, handles);
    }
    const handle = target.requestAnimationFrame((timestamp) => {
      handles.delete(handle);
      const exports = getWasmExports();
      const trampoline = exports?.[CALLBACK_F64_UNIT_TRAMPOLINE_EXPORT];
      if (typeof trampoline !== 'function') {
        throw new Error(
          `Missing ${CALLBACK_F64_UNIT_TRAMPOLINE_EXPORT} export for animation frame callback`
        );
      }
      trampoline(callback, timestamp);
    });
    handles.add(handle);
    return handle;
  };

  const fetchFromDomContext = (input) => {
    // Use globalThis.fetch rather than the iframe window's fetch.  The fetch
    // host import is called from the host-page JS context, not from inside the
    // iframe, so tying it to the iframe window would cause the request to be
    // cancelled by the browser if the iframe is removed before the promise
    // settles (e.g. in the conformance runner's temp-iframe teardown path).
    const fetchImpl = globalThis.fetch ?? outputDocument()?.defaultView?.fetch;
    if (typeof fetchImpl !== 'function') {
      throw new Error('fetch is not available in this browser context');
    }
    return fetchImpl(String(input));
  };

  const getProperty = (interfaceName, propertyName, receiver) => {
    if (interfaceName === 'Window' && propertyName === 'localStorage') {
      return playgroundStorage();
    }
    return receiver[resolveDomMemberName(receiver, propertyName)];
  };

  const setProperty = (_interfaceName, propertyName, receiver, value) => {
    receiver[resolveDomMemberName(receiver, propertyName)] = value;
  };

  const forwardMethod = (_interfaceName, methodName, receiver, args) => {
    return receiver[resolveDomMemberName(receiver, methodName)](...args);
  };

  const specialImports = {
    dom_window: outputWindow,
    fetch: fetchFromDomContext,
    // Curated 3D context acquisition (waluau-9tvw): the generated
    // get_context_webgl2 extern member (imported under its camelized wasm
    // name) has no matching JS method, so it forwards to
    // getContext("webgl2") explicitly. preserveDrawingBuffer keeps the frame
    // readable after the task that drew it, so conformance tests can verify
    // rendering with readPixels.
    'HTMLCanvasElement.getContextWebgl2': (canvas) =>
      canvas.getContext('webgl2', { preserveDrawingBuffer: true }),
    // Wrap a linear-memory typed array (an i32 data pointer; the element
    // count lives in the 8-byte allocation header preceding the data) as a
    // JS Float32Array view for BufferSource-typed extern params like
    // WebGL2's bufferData. The view is created fresh on every call because
    // memory growth detaches previous ArrayBuffer views.
    dom_float32_array_view: (pointer) => {
      const memory = getWasmMemory();
      if (!(memory instanceof WebAssembly.Memory)) {
        throw new Error('dom_float32_array_view requires the waluau memory import');
      }
      const length = new DataView(memory.buffer).getUint32(pointer - 8, true);
      return new Float32Array(memory.buffer, pointer, length);
    },
    'EventTarget.addEventListener': (target, type, callback) => registerEventListener(target, String(type), callback),
    'Window.requestAnimationFrame': requestAnimationFrame,
    'Node.removeChild': removeChild,
    'Node.replaceChild': replaceChild,
    'Window.get/localStorage': () => playgroundStorage(),
  };

  const domImports = {};
  for (const wasmImport of wasmImports) {
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

class GameServiceError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

function memoryStorage() {
  const values = new Map();
  return {
    getItem(key) {
      const normalized = String(key);
      return values.has(normalized) ? values.get(normalized) : null;
    },
    setItem(key, value) {
      values.set(String(key), String(value));
    },
    removeItem(key) {
      values.delete(String(key));
    },
  };
}

function encodeBase64(bytes) {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

function decodeBase64(value) {
  const binary = atob(String(value));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function validatedAssetPath(path) {
  const value = String(path);
  if (
    value.length === 0 ||
    value.startsWith('//') ||
    value.includes('\0') ||
    value.includes('\\') ||
    value.includes('?') ||
    value.includes('#') ||
    /^[a-z][a-z0-9+.-]*:/i.test(value) ||
    /%(?:2e|2f|5c)/i.test(value) ||
    value.split('/').includes('..')
  ) {
    throw new GameServiceError('invalid_path', `invalid packaged asset path: ${value}`);
  }
  return value;
}

function validatedSavePart(value, label) {
  const normalized = String(value);
  if (!/^[A-Za-z0-9._-]{1,128}$/.test(normalized)) {
    throw new GameServiceError('invalid_key', `invalid save ${label}: ${normalized}`);
  }
  return normalized;
}

function serviceErrorCode(error, fallback) {
  if (error instanceof GameServiceError) return error.code;
  if (error?.name === 'NotAllowedError') return 'permission_denied';
  if (error?.name === 'QuotaExceededError') return 'storage_full';
  return fallback;
}

// Browser implementation of engine/resources.walu, audio.walu and save.walu.
// Dependencies are injectable so the browser conformance suite can exercise
// decoding, playback and storage deterministically without network or devices.
export function createGameServicesHost(options = {}) {
  const document = options.document ?? globalThis.document;
  const fetchImpl = options.fetch ?? globalThis.fetch;
  const createBitmap = options.createImageBitmap ?? globalThis.createImageBitmap;
  const ImageCtor = options.Image ?? document?.defaultView?.Image ?? globalThis.Image;
  const createObjectUrl =
    options.createObjectURL ?? globalThis.URL?.createObjectURL?.bind(globalThis.URL);
  const revokeObjectUrl =
    options.revokeObjectURL ?? globalThis.URL?.revokeObjectURL?.bind(globalThis.URL);
  const FontFaceCtor = options.FontFace ?? globalThis.FontFace;
  const fontSet = options.fontSet ?? document?.fonts;
  const createFontAtlas = options.createFontAtlas ?? ((face) => {
    const firstCode = 32;
    const glyphCount = 95;
    const columns = 16;
    const cellWidth = 40;
    const cellHeight = 44;
    const baseSize = 32;
    const advance = 32;
    const rows = Math.ceil(glyphCount / columns);
    const canvas = document?.createElement?.('canvas');
    const context = canvas?.getContext?.('2d');
    if (!canvas || !context) {
      throw new GameServiceError('unavailable', 'font atlas canvas is unavailable');
    }
    canvas.width = columns * cellWidth;
    canvas.height = rows * cellHeight;
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.fillStyle = '#ffffff';
    context.font = `${baseSize}px ${JSON.stringify(String(face.family))}`;
    context.textBaseline = 'top';
    context.textAlign = 'left';
    for (let offset = 0; offset < glyphCount; offset += 1) {
      const column = offset % columns;
      const row = Math.floor(offset / columns);
      context.fillText(String.fromCharCode(firstCode + offset), column * cellWidth + 4, row * cellHeight + 4);
    }
    return {
      source: canvas,
      width: canvas.width,
      height: canvas.height,
      firstCode,
      glyphCount,
      columns,
      cellWidth,
      cellHeight,
      baseSize,
      advance,
    };
  });
  const AudioContextCtor =
    options.AudioContext ?? globalThis.AudioContext ?? globalThis.webkitAudioContext;
  const AudioCtor = options.Audio ?? globalThis.Audio;
  const baseUrl = options.assetBaseUrl ?? document?.baseURI ?? globalThis.location?.href;
  const assetManifest = options.assetManifest == null
    ? null
    : new Map(options.assetManifest instanceof Map
      ? options.assetManifest
      : Object.entries(options.assetManifest));
  const storage = options.storage ?? (() => {
    try {
      return document?.defaultView?.localStorage ?? globalThis.localStorage;
    } catch {
      return null;
    }
  })() ?? memoryStorage();
  const saveNamespace = validatedSavePart(options.saveNamespace ?? 'default', 'namespace');

  const resources = new Map();
  const gpuResources = new Map();
  const releasedGpuResources = new Set();
  let nextHandle = 1;
  let nextGpuHandle = 1;
  let audioContext = null;

  const remember = (entry) => {
    const handle = nextHandle++;
    resources.set(handle, entry);
    return handle;
  };
  const success = (kind, value, extra = {}) => remember({ ok: true, kind, value, ...extra });
  const failure = (path, error, fallbackCode) => remember({
    ok: false,
    kind: 'error',
    path,
    code: serviceErrorCode(error, fallbackCode),
    message: error instanceof Error ? error.message : String(error),
  });
  const entryFor = (handle) => resources.get(Number(handle));
  const rememberGpu = (entry) => {
    const handle = nextGpuHandle++;
    gpuResources.set(handle, entry);
    return handle;
  };
  const gpuFailure = (code, message) => rememberGpu({ ok: false, code, message });
  const gpuEntryFor = (handle) => gpuResources.get(Number(handle));
  const gpuErrorCode = (handle) => {
    const normalized = Number(handle);
    if (releasedGpuResources.has(normalized)) return 'released';
    const entry = gpuResources.get(normalized);
    return entry?.ok ? '' : (entry?.code ?? 'invalid_handle');
  };
  const gpuErrorMessage = (handle) => {
    const code = gpuErrorCode(handle);
    if (!code) return '';
    return gpuEntryFor(handle)?.message ?? `${code} GPU resource: ${handle}`;
  };
  const resolveAssetUrl = (path, expectedType) => {
    const normalized = validatedAssetPath(path);
    if (assetManifest) {
      if (!assetManifest.has(normalized)) {
        throw new GameServiceError(
          'undeclared_asset',
          `packaged asset is not declared in the manifest: ${normalized}`
        );
      }
      const entry = assetManifest.get(normalized);
      const emittedUrl = typeof entry === 'string' ? entry : entry?.url;
      const declaredType = typeof entry === 'string' ? null : entry?.type;
      if (typeof emittedUrl !== 'string' || emittedUrl.length === 0) {
        throw new GameServiceError('invalid_manifest', `invalid manifest URL for: ${normalized}`);
      }
      if (declaredType && declaredType !== expectedType) {
        throw new GameServiceError(
          'wrong_asset_type',
          `asset ${normalized} is declared as ${declaredType}, not ${expectedType}`
        );
      }
      return baseUrl ? new URL(emittedUrl, baseUrl).href : emittedUrl;
    }
    return baseUrl ? new URL(normalized, baseUrl).href : normalized;
  };
  const fetchAsset = async (path, expectedType) => {
    const normalized = validatedAssetPath(path);
    if (typeof fetchImpl !== 'function') {
      throw new GameServiceError('unavailable', 'fetch is unavailable in this host');
    }
    const url = resolveAssetUrl(normalized, expectedType);
    const response = await fetchImpl(url);
    if (!response?.ok) {
      const status = Number(response?.status);
      const code = status === 404 ? 'not_found' : 'http_error';
      throw new GameServiceError(code, `asset fetch failed (${status || 'unknown'}): ${normalized}`);
    }
    return response;
  };
  const load = async (kind, path, decode) => {
    try {
      const expectedType = kind === 'sound' || kind === 'stream' ? 'audio' : kind;
      const response = await fetchAsset(path, expectedType);
      return success(kind, await decode(response));
    } catch (error) {
      return failure(String(path), error, 'decode_failed');
    }
  };
  const decodeImage = async (response) => {
    const blob = await response.blob();
    let bitmapError = null;
    if (typeof createBitmap === 'function') {
      try {
        return await createBitmap(blob);
      } catch (error) {
        bitmapError = error;
      }
    }
    // Chromium cannot createImageBitmap() directly from every SVG blob even
    // though its image decoder can load the same source. Keep the decoded
    // HTMLImageElement as the TexImageSource fallback; texImage2D accepts both.
    if (
      typeof ImageCtor !== 'function' ||
      typeof createObjectUrl !== 'function' ||
      typeof revokeObjectUrl !== 'function'
    ) {
      if (bitmapError) throw bitmapError;
      throw new GameServiceError('unavailable', 'no browser image decoder is available');
    }
    const url = createObjectUrl(blob);
    try {
      const image = new ImageCtor();
      if (typeof image.decode === 'function') {
        image.src = url;
        await image.decode();
      } else {
        await new Promise((resolve, reject) => {
          image.onload = resolve;
          image.onerror = () => reject(new Error('the source image could not be decoded'));
          image.src = url;
        });
      }
      return image;
    } finally {
      revokeObjectUrl(url);
    }
  };
  const context = () => {
    if (typeof AudioContextCtor !== 'function') {
      throw new GameServiceError('unavailable', 'Web Audio is unavailable in this host');
    }
    audioContext ??= new AudioContextCtor();
    return audioContext;
  };
  const saveKey = (slot) =>
    `waluau.save.v1:${saveNamespace}:${validatedSavePart(slot, 'slot')}`;
  const saveOperation = async (slot, operation) => {
    try {
      const key = saveKey(slot);
      return success('save-operation', await operation(key));
    } catch (error) {
      return failure(String(slot), error, 'storage_failed');
    }
  };

  const release = (handle) => {
    const normalized = Number(handle);
    const entry = resources.get(normalized);
    if (!entry) return;
    resources.delete(normalized);
    if (!entry.ok) return;
    if (entry.kind === 'image') entry.value?.close?.();
    if (entry.kind === 'font') fontSet?.delete?.(entry.value);
    if (entry.kind === 'sound') {
      for (const source of entry.sources) {
        try { source.stop(); } catch { /* already stopped */ }
        source.disconnect?.();
      }
      entry.sources.clear();
    }
    if (entry.kind === 'stream') {
      entry.value.pause?.();
      entry.value.removeAttribute?.('src');
      entry.value.load?.();
    }
  };

  const createGpuTexture = (gl, resourceHandle) => {
    const source = entryFor(resourceHandle);
    if (!source?.ok) return gpuFailure('invalid_resource', `invalid image resource: ${resourceHandle}`);
    if (source.kind !== 'image') return gpuFailure('wrong_type', `resource is ${source.kind}, expected image`);
    let texture = null;
    try {
      texture = gl?.createTexture?.();
      if (!texture) return gpuFailure('unavailable', 'WebGL texture creation is unavailable');
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, true);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source.value);
      // texImage2D copies the decoded pixels, so releasing the source bitmap
      // cannot invalidate this GPU resource.
      return rememberGpu({
        ok: true, kind: 'texture', gl, texture,
        width: Number(source.value.width), height: Number(source.value.height),
      });
    } catch (error) {
      gl?.deleteTexture?.(texture);
      return gpuFailure('upload_failed', error instanceof Error ? error.message : String(error));
    }
  };
  const createGpuFont = (gl, resourceHandle) => {
    const source = entryFor(resourceHandle);
    if (!source?.ok) return gpuFailure('invalid_resource', `invalid font resource: ${resourceHandle}`);
    if (source.kind !== 'font') return gpuFailure('wrong_type', `resource is ${source.kind}, expected font`);
    let texture = null;
    try {
      const atlas = createFontAtlas(source.value);
      if (!atlas?.source || !Number.isFinite(atlas.width) || !Number.isFinite(atlas.height)) {
        return gpuFailure('atlas_failed', 'font atlas generator returned invalid metadata');
      }
      texture = gl?.createTexture?.();
      if (!texture) return gpuFailure('unavailable', 'WebGL font texture creation is unavailable');
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, true);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, atlas.source);
      return rememberGpu({
        ok: true,
        kind: 'font',
        gl,
        texture,
        width: atlas.width,
        height: atlas.height,
        firstCode: atlas.firstCode,
        glyphCount: atlas.glyphCount,
        columns: atlas.columns,
        cellWidth: atlas.cellWidth,
        cellHeight: atlas.cellHeight,
        baseSize: atlas.baseSize,
        advance: atlas.advance,
      });
    } catch (error) {
      gl?.deleteTexture?.(texture);
      const code = error instanceof GameServiceError ? error.code : 'atlas_failed';
      return gpuFailure(code, error instanceof Error ? error.message : String(error));
    }
  };
  const createRenderTarget = (gl, width, height) => {
    const w = Number(width);
    const h = Number(height);
    if (!Number.isInteger(w) || !Number.isInteger(h) || w <= 0 || h <= 0) {
      return gpuFailure('invalid_size', `invalid render target size: ${width}x${height}`);
    }
    let texture = null;
    let framebuffer = null;
    try {
      texture = gl?.createTexture?.();
      framebuffer = gl?.createFramebuffer?.();
      if (!texture || !framebuffer) {
        gl?.deleteFramebuffer?.(framebuffer);
        gl?.deleteTexture?.(texture);
        return gpuFailure('unavailable', 'WebGL render targets are unavailable');
      }
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
      gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, texture, 0);
      const complete = gl.checkFramebufferStatus(gl.FRAMEBUFFER) === gl.FRAMEBUFFER_COMPLETE;
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      if (!complete) {
        gl.deleteFramebuffer(framebuffer);
        gl.deleteTexture(texture);
        return gpuFailure('incomplete_target', 'WebGL framebuffer is incomplete');
      }
      return rememberGpu({ ok: true, kind: 'render-target', gl, texture, framebuffer, width: w, height: h });
    } catch (error) {
      gl?.deleteFramebuffer?.(framebuffer);
      gl?.deleteTexture?.(texture);
      return gpuFailure('creation_failed', error instanceof Error ? error.message : String(error));
    }
  };
  const releaseGpu = (handle) => {
    const normalized = Number(handle);
    const entry = gpuResources.get(normalized);
    if (!entry) return;
    gpuResources.delete(normalized);
    releasedGpuResources.add(normalized);
    if (!entry.ok) return;
    if (entry.framebuffer) entry.gl.deleteFramebuffer?.(entry.framebuffer);
    if (entry.texture) entry.gl.deleteTexture?.(entry.texture);
  };

  return {
    game_resource_load_text: (path) => load('text', path, (response) => response.text()),
    game_resource_load_bytes: (path) =>
      load('bytes', path, async (response) => new Uint8Array(await response.arrayBuffer())),
    game_resource_load_image: (path) => load('image', path, decodeImage),
    game_resource_load_font: (path, family) => load('font', path, async (response) => {
      if (typeof FontFaceCtor !== 'function' || !fontSet?.add) {
        throw new GameServiceError('unavailable', 'FontFace loading is unavailable in this host');
      }
      const face = new FontFaceCtor(String(family), await response.arrayBuffer());
      const loaded = await face.load();
      fontSet.add(loaded);
      return loaded;
    }),
    game_resource_ok: (handle) => Boolean(entryFor(handle)?.ok),
    game_resource_error_code: (handle) => {
      const entry = entryFor(handle);
      return entry?.ok ? '' : (entry?.code ?? 'invalid_handle');
    },
    game_resource_error_message: (handle) => {
      const entry = entryFor(handle);
      return entry?.ok ? '' : (entry?.message ?? `unknown resource handle: ${handle}`);
    },
    game_resource_text: (handle) => {
      const entry = entryFor(handle);
      return entry?.ok && (entry.kind === 'text' || entry.kind === 'save-text')
        ? String(entry.value)
        : '';
    },
    game_resource_bytes: (handle) => {
      const entry = entryFor(handle);
      return entry?.ok && (entry.kind === 'bytes' || entry.kind === 'save-bytes')
        ? entry.value.slice()
        : new Uint8Array();
    },
    game_resource_font_family: (handle) => {
      const entry = entryFor(handle);
      return entry?.ok && entry.kind === 'font' ? String(entry.value.family ?? '') : '';
    },
    game_resource_release: release,
    game_gpu_texture_from_resource: createGpuTexture,
    game_gpu_font_from_resource: createGpuFont,
    game_gpu_render_target_create: createRenderTarget,
    game_gpu_resource_ok: (handle) => Boolean(gpuEntryFor(handle)?.ok),
    game_gpu_resource_error_code: gpuErrorCode,
    game_gpu_resource_error_message: gpuErrorMessage,
    game_gpu_resource_width: (handle) => Number(gpuEntryFor(handle)?.width ?? 0),
    game_gpu_resource_height: (handle) => Number(gpuEntryFor(handle)?.height ?? 0),
    game_gpu_font_first_code: (handle) => Number(gpuEntryFor(handle)?.firstCode ?? 0),
    game_gpu_font_glyph_count: (handle) => Number(gpuEntryFor(handle)?.glyphCount ?? 0),
    game_gpu_font_columns: (handle) => Number(gpuEntryFor(handle)?.columns ?? 0),
    game_gpu_font_cell_width: (handle) => Number(gpuEntryFor(handle)?.cellWidth ?? 0),
    game_gpu_font_cell_height: (handle) => Number(gpuEntryFor(handle)?.cellHeight ?? 0),
    game_gpu_font_base_size: (handle) => Number(gpuEntryFor(handle)?.baseSize ?? 0),
    game_gpu_font_advance: (handle) => Number(gpuEntryFor(handle)?.advance ?? 0),
    game_gpu_texture_bind: (gl, handle) => {
      const entry = gpuEntryFor(handle);
      if (!entry?.ok || entry.gl !== gl) return false;
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, entry.texture);
      return true;
    },
    game_gpu_target_bind: (gl, handle) => {
      const entry = gpuEntryFor(handle);
      if (!entry?.ok || entry.kind !== 'render-target' || entry.gl !== gl) return false;
      gl.bindFramebuffer(gl.FRAMEBUFFER, entry.framebuffer);
      gl.viewport(0, 0, entry.width, entry.height);
      return true;
    },
    game_gpu_screen_bind: (gl) => {
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
    },
    game_gpu_resource_release: releaseGpu,

    game_audio_load_sound: (path) => load('sound', path, async (response) => {
      const ctx = context();
      const buffer = await ctx.decodeAudioData(await response.arrayBuffer());
      return { buffer, context: ctx };
    }).then((handle) => {
      const entry = entryFor(handle);
      if (entry?.ok) {
        entry.sources = new Set();
        entry.context = entry.value.context;
        entry.value = entry.value.buffer;
      }
      return handle;
    }),
    game_audio_load_stream: async (path) => {
      try {
        const normalized = validatedAssetPath(path);
        if (typeof AudioCtor !== 'function') {
          throw new GameServiceError('unavailable', 'streaming audio is unavailable in this host');
        }
        const url = resolveAssetUrl(normalized, 'audio');
        const audio = new AudioCtor(url);
        audio.preload = 'auto';
        await new Promise((resolve, reject) => {
          if (audio.readyState >= 3) {
            resolve();
            return;
          }
          audio.addEventListener('canplaythrough', resolve, { once: true });
          audio.addEventListener('error', () => reject(
            new GameServiceError('decode_failed', `streaming audio failed: ${normalized}`)
          ), { once: true });
          audio.load?.();
        });
        return success('stream', audio);
      } catch (error) {
        return failure(String(path), error, 'decode_failed');
      }
    },
    game_audio_play: (handle, looped, volume) => {
      const entry = entryFor(handle);
      if (!entry?.ok) return false;
      const normalizedVolume = Math.max(0, Math.min(1, Number(volume)));
      try {
        if (entry.kind === 'sound') {
          const source = entry.context.createBufferSource();
          const gain = entry.context.createGain();
          source.buffer = entry.value;
          source.loop = Boolean(looped);
          gain.gain.value = normalizedVolume;
          source.connect(gain);
          gain.connect(entry.context.destination);
          source.onended = () => entry.sources.delete(source);
          entry.sources.add(source);
          entry.context.resume?.().catch?.(() => {});
          source.start();
          return true;
        }
        if (entry.kind === 'stream') {
          entry.value.loop = Boolean(looped);
          entry.value.volume = normalizedVolume;
          entry.value.play?.().catch?.(() => {});
          return true;
        }
      } catch {
        return false;
      }
      return false;
    },
    game_audio_pause: (handle) => {
      const entry = entryFor(handle);
      if (entry?.ok && entry.kind === 'stream') entry.value.pause?.();
    },
    game_audio_stop: (handle) => {
      const entry = entryFor(handle);
      if (!entry?.ok) return;
      if (entry.kind === 'sound') {
        for (const source of entry.sources) {
          try { source.stop(); } catch { /* already stopped */ }
        }
        entry.sources.clear();
      } else if (entry.kind === 'stream') {
        entry.value.pause?.();
        entry.value.currentTime = 0;
      }
    },
    game_audio_is_playing: (handle) => {
      const entry = entryFor(handle);
      if (!entry?.ok) return false;
      if (entry.kind === 'sound') return entry.sources.size > 0;
      return entry.kind === 'stream' && !entry.value.paused && !entry.value.ended;
    },

    game_save_read_text: (slot) => saveOperation(slot, (key) => {
      const value = storage.getItem(key);
      if (value == null) throw new GameServiceError('not_found', `save slot not found: ${slot}`);
      if (!value.startsWith('text:')) {
        throw new GameServiceError('wrong_type', `save slot does not contain text: ${slot}`);
      }
      return value.slice(5);
    }).then((handle) => {
      const entry = entryFor(handle);
      if (entry?.ok) entry.kind = 'save-text';
      return handle;
    }),
    game_save_read_bytes: (slot) => saveOperation(slot, (key) => {
      const value = storage.getItem(key);
      if (value == null) throw new GameServiceError('not_found', `save slot not found: ${slot}`);
      if (!value.startsWith('bytes:')) {
        throw new GameServiceError('wrong_type', `save slot does not contain bytes: ${slot}`);
      }
      return decodeBase64(value.slice(6));
    }).then((handle) => {
      const entry = entryFor(handle);
      if (entry?.ok) entry.kind = 'save-bytes';
      return handle;
    }),
    game_save_write_text: (slot, value) => saveOperation(slot, (key) => {
      storage.setItem(key, `text:${String(value)}`);
      return null;
    }),
    game_save_write_bytes: (slot, value) => saveOperation(slot, (key) => {
      storage.setItem(key, `bytes:${encodeBase64(value)}`);
      return null;
    }),
    game_save_delete: (slot) => saveOperation(slot, (key) => {
      storage.removeItem(key);
      return null;
    }),
  };
}

function createTfjsHost(getWasmExports = () => null) {
  const graphModels = new Map();
  const layersModels = new Map();
  const disposedModels = new Set();
  const tensors = new Map();
  const trainingHistories = new Map();
  let nextTensorHandle = 1;
  let nextModelHandle = 1_000_000;
  let nextTrainingHistoryHandle = 2_000_000;
  let currentTensor = null;
  let currentGraphModel = null;
  let currentLayersModel = null;

  // Promise-resolved externrefs can lose object identity when they cross the
  // coroutine bridge, so async TFJS results are retained by host handle.
  const ensureTfjs = () => {
    if (!tf || typeof tf.tensor !== 'function') {
      throw new Error('TensorFlow.js is not available for require("tfjs")');
    }
    return tf;
  };

  const isTensor = (value) => Boolean(value && typeof value === 'object' && value.isDisposedInternal !== undefined && Array.isArray(value.shape));
  const isHostHandle = (value) => typeof value === 'number' && Number.isFinite(value);
  const rememberTensor = (tensor) => {
    if (!isTensor(tensor)) return tensor;
    const handle = nextTensorHandle++;
    tensors.set(handle, tensor);
    currentTensor = tensor;
    return handle;
  };
  const asTensor = (value) => {
    const remembered = tensors.get(Number(value));
    const checked = remembered ?? (isHostHandle(value) ? null : (isTensor(value) ? value : currentTensor));
    if (!checked) {
      throw new TypeError('Expected TensorFlow.js Tensor host object');
    }
    if (checked.isDisposedInternal) {
      throw new Error('TensorFlow.js Tensor has been disposed');
    }
    return checked;
  };

  const isObject = (value) => Boolean(value && typeof value === 'object');

  const rememberGraphModel = (model) => {
    if (!isObject(model)) {
      throw new TypeError('tf.loadGraphModel did not return a model object');
    }
    const handle = nextModelHandle++;
    graphModels.set(handle, model);
    currentGraphModel = model;
    return handle;
  };

  const rememberLayersModel = (model) => {
    if (!isObject(model)) {
      throw new TypeError('tf.loadLayersModel did not return a model object');
    }
    const handle = nextModelHandle++;
    layersModels.set(handle, model);
    currentLayersModel = model;
    return handle;
  };

  const asGraphModel = (value) => {
    const remembered = graphModels.get(Number(value));
    const isGraphModelLike = isObject(value) &&
      typeof value.predict === 'function' &&
      typeof value.predictAsync === 'function' &&
      typeof value.execute === 'function' &&
      Array.isArray(value.inputs) &&
      Array.isArray(value.outputs);
    const fallback = isHostHandle(value) ? null : currentGraphModel;
    if (!remembered && !isGraphModelLike && !fallback) {
      throw new TypeError('Expected TensorFlow.js GraphModel host object');
    }
    const model = remembered ?? (isGraphModelLike ? value : fallback);
    if (disposedModels.has(Number(value)) || disposedModels.has(model)) {
      throw new Error('TensorFlow.js GraphModel has been disposed');
    }
    return model;
  };

  const asLayersModel = (value) => {
    const remembered = layersModels.get(Number(value));
    const isLayersModelLike = isObject(value) &&
      typeof value.predict === 'function' &&
      Array.isArray(value.inputs) &&
      Array.isArray(value.outputs);
    const fallback = isHostHandle(value) ? null : currentLayersModel;
    if (!remembered && !isLayersModelLike && !fallback) {
      throw new TypeError('Expected TensorFlow.js LayersModel host object');
    }
    const model = remembered ?? (isLayersModelLike ? value : fallback);
    if (disposedModels.has(Number(value)) || disposedModels.has(model)) {
      throw new Error('TensorFlow.js LayersModel has been disposed');
    }
    return model;
  };

  const asSingleOutputTensor = (value, apiName) => {
    if (isTensor(value)) return asTensor(value);
    if (Array.isArray(value)) {
      throw new Error(`${apiName} returned multiple outputs; the Waluau TFJS model API only supports single-output models`);
    }
    if (isObject(value)) {
      throw new Error(`${apiName} returned a named output map; the Waluau TFJS model API only supports single-output models`);
    }
    throw new TypeError(`${apiName} did not return a TensorFlow.js Tensor`);
  };

  const modelCount = (model, propertyName, modelName) => {
    const value = model[propertyName];
    if (!Array.isArray(value)) {
      throw new Error(`${modelName}.${propertyName} is not available`);
    }
    return value.length;
  };

  const checkPositiveFinite = (value, name) => {
    const n = Number(value);
    if (!Number.isFinite(n) || n <= 0) {
      throw new RangeError(`${name} must be a finite positive number, got ${value}`);
    }
    return n;
  };

  const checkPositiveInteger = (value, name) => {
    const n = Number(value);
    if (!Number.isInteger(n) || n <= 0) {
      throw new RangeError(`${name} must be a positive integer, got ${value}`);
    }
    return n;
  };

  const assertBuiltinLossName = (tfjs, loss) => {
    const name = String(loss);
    if (!name) {
      throw new TypeError('TFJS loss name must be a non-empty string');
    }
    if (!tfjs.losses || typeof tfjs.losses[name] !== 'function') {
      throw new RangeError(`Unknown TensorFlow.js built-in loss: ${name}`);
    }
    return name;
  };

  const rememberTrainingHistory = (history) => {
    if (!isObject(history) || !isObject(history.history)) {
      throw new TypeError('tf.LayersModel.fit did not return a TrainingHistory object');
    }
    const handle = nextTrainingHistoryHandle++;
    trainingHistories.set(handle, history);
    return handle;
  };

  const asTrainingHistory = (value) => {
    const remembered = trainingHistories.get(Number(value));
    const checked = remembered ?? (isHostHandle(value) ? null : value);
    if (!isObject(checked) || !isObject(checked.history)) {
      throw new TypeError('Expected TensorFlow.js TrainingHistory host object');
    }
    return checked;
  };

  const lossHistory = (history) => {
    const checked = asTrainingHistory(history);
    const losses = checked.history.loss;
    if (!Array.isArray(losses)) {
      throw new Error('TrainingHistory is missing numeric loss history');
    }
    for (let i = 0; i < losses.length; i += 1) {
      if (!Number.isFinite(Number(losses[i]))) {
        throw new Error(`TrainingHistory loss at index ${i} is not numeric`);
      }
    }
    return losses;
  };

  const historyLossAt = (history, index) => {
    const losses = lossHistory(history);
    const i = Number(index);
    if (!Number.isInteger(i) || i < 0 || i >= losses.length) {
      throw new RangeError(`TrainingHistory loss index out of bounds: ${index}`);
    }
    return Number(losses[i]);
  };

  const makeTensorData = (values, dtype = 'float32') => ({
    __waluauTfjsTensorData: true,
    dtype,
    values: Array.from(values, (value) => Number(value)),
  });

  const asTensorData = (value) => {
    if (!value || value.__waluauTfjsTensorData !== true || !Array.isArray(value.values)) {
      throw new TypeError('Expected TensorData host object');
    }
    return value;
  };

  const checkDataIndex = (data, index) => {
    const i = Number(index);
    if (!Number.isInteger(i) || i < 0 || i >= data.values.length) {
      throw new RangeError(`TensorData index out of bounds: ${index}`);
    }
    return i;
  };

  const checkLength = (length) => {
    const n = Number(length);
    if (!Number.isInteger(n) || n < 0) {
      throw new RangeError(`TensorData length must be a non-negative integer, got ${length}`);
    }
    return n;
  };

  const checkDim = (dim, name) => {
    const n = Number(dim);
    if (!Number.isInteger(n) || n < 0) {
      throw new RangeError(`${name} must be a non-negative integer, got ${dim}`);
    }
    return n;
  };

  const tensorFromData = (data, shape, dtype) => {
    const tfjs = ensureTfjs();
    const tensorData = asTensorData(data);
    const expected = shape.reduce((product, dim) => product * dim, 1);
    if (tensorData.values.length !== expected) {
      throw new RangeError(`TensorData length ${tensorData.values.length} does not match shape [${shape.join(', ')}]`);
    }
    return rememberTensor(tfjs.tensor(tensorData.values, shape, dtype));
  };

  const scalarValue = (tensor) => {
    const checked = asTensor(tensor);
    if (checked.rank !== 0) {
      throw new Error(`Expected scalar Tensor, got rank ${checked.rank}`);
    }
    return checked.dataSync()[0];
  };

  const dataFromTensor = (tensor) => {
    const checked = asTensor(tensor);
    return makeTensorData(checked.dataSync(), checked.dtype);
  };

  const callUnitExternCallback = (callback) => {
    const exports = getWasmExports();
    const trampoline = exports?.[CALLBACK_UNIT_EXTERN_TRAMPOLINE_EXPORT];
    if (typeof trampoline !== 'function') {
      throw new Error(`Missing ${CALLBACK_UNIT_EXTERN_TRAMPOLINE_EXPORT} export for synchronous host callback`);
    }
    return trampoline(callback);
  };

  return {
    tfjs_data_empty: (length) => makeTensorData(new Array(checkLength(length)).fill(0)),
    tfjs_data_set_f64: (data, index, value) => {
      const tensorData = asTensorData(data);
      tensorData.values[checkDataIndex(tensorData, index)] = Number(value);
    },
    tfjs_data_set_i32: (data, index, value) => {
      const tensorData = asTensorData(data);
      tensorData.values[checkDataIndex(tensorData, index)] = Number(value) | 0;
      tensorData.dtype = 'int32';
    },
    tfjs_data_len: (data) => asTensorData(data).values.length,
    tfjs_data_get_f64: (data, index) => asTensorData(data).values[checkDataIndex(asTensorData(data), index)],
    tfjs_data_get_i32: (data, index) => asTensorData(data).values[checkDataIndex(asTensorData(data), index)] | 0,
    tfjs_scalar: (value) => rememberTensor(ensureTfjs().scalar(Number(value), 'float32')),
    tfjs_scalar_i32: (value) => rememberTensor(ensureTfjs().scalar(Number(value) | 0, 'int32')),
    tfjs_scalar_bool: (value) => rememberTensor(ensureTfjs().scalar(Boolean(value), 'bool')),
    tfjs_tensor1d: (data) => tensorFromData(data, [asTensorData(data).values.length], 'float32'),
    tfjs_tensor1d_i32: (data) => tensorFromData(data, [asTensorData(data).values.length], 'int32'),
    tfjs_tensor2d: (data, rows, cols) => tensorFromData(data, [checkDim(rows, 'rows'), checkDim(cols, 'cols')], 'float32'),
    tfjs_tensor2d_i32: (data, rows, cols) => tensorFromData(data, [checkDim(rows, 'rows'), checkDim(cols, 'cols')], 'int32'),
    tfjs_zeros: (rows, cols) => rememberTensor(ensureTfjs().zeros([checkDim(rows, 'rows'), checkDim(cols, 'cols')], 'float32')),
    tfjs_ones: (rows, cols) => rememberTensor(ensureTfjs().ones([checkDim(rows, 'rows'), checkDim(cols, 'cols')], 'float32')),
    tfjs_eye: (size) => {
      const n = checkDim(size, 'size');
      return rememberTensor(ensureTfjs().eye(n, n));
    },
    tfjs_data: async (tensor) => {
      const checked = asTensor(tensor);
      const dtype = checked.dtype;
      return makeTensorData(await checked.data(), dtype);
    },
    tfjs_data_sync: dataFromTensor,
    tfjs_scalar_value: (tensor) => Number(scalarValue(tensor)),
    tfjs_scalar_value_i32: (tensor) => Number(scalarValue(tensor)) | 0,
    tfjs_shape_rank: (tensor) => asTensor(tensor).rank,
    tfjs_shape_dim: (tensor, index) => {
      const checked = asTensor(tensor);
      const i = Number(index);
      if (!Number.isInteger(i) || i < 0 || i >= checked.shape.length) {
        throw new RangeError(`Tensor shape index out of bounds: ${index}`);
      }
      return checked.shape[i];
    },
    tfjs_dtype: (tensor) => asTensor(tensor).dtype,
    tfjs_dispose: (tensor) => {
      const checked = tensors.get(Number(tensor)) ?? (isHostHandle(tensor) ? null : (isTensor(tensor) ? tensor : currentTensor));
      if (!checked) {
        throw new TypeError('Expected TensorFlow.js Tensor host object');
      }
      if (!checked.isDisposedInternal) {
        checked.dispose();
      }
    },
    tfjs_keep: (tensor) => rememberTensor(ensureTfjs().keep(asTensor(tensor))),
    tfjs_tidy: (callback) => rememberTensor(ensureTfjs().tidy(() => asTensor(callUnitExternCallback(callback)))),
    tfjs_memory_num_tensors: () => ensureTfjs().memory().numTensors,
    tfjs_add: (left, right) => rememberTensor(ensureTfjs().add(asTensor(left), asTensor(right))),
    tfjs_sub: (left, right) => rememberTensor(ensureTfjs().sub(asTensor(left), asTensor(right))),
    tfjs_mul: (left, right) => rememberTensor(ensureTfjs().mul(asTensor(left), asTensor(right))),
    tfjs_div: (left, right) => rememberTensor(ensureTfjs().div(asTensor(left), asTensor(right))),
    tfjs_neg: (tensor) => rememberTensor(ensureTfjs().neg(asTensor(tensor))),
    tfjs_matmul: (left, right) => rememberTensor(ensureTfjs().matMul(asTensor(left), asTensor(right))),
    tfjs_reshape2d: (tensor, rows, cols) =>
      rememberTensor(asTensor(tensor).reshape([checkDim(rows, 'rows'), checkDim(cols, 'cols')])),
    tfjs_transpose: (tensor) => rememberTensor(ensureTfjs().transpose(asTensor(tensor))),
    tfjs_load_graph_model: async (url) => rememberGraphModel(await ensureTfjs().loadGraphModel(String(url))),
    tfjs_load_layers_model: async (url) => rememberLayersModel(await ensureTfjs().loadLayersModel(String(url))),
    tfjs_dispose_graph_model: (model) => {
      const checked = asGraphModel(model);
      if (typeof checked.dispose !== 'function') {
        throw new TypeError('TensorFlow.js GraphModel does not support dispose()');
      }
      checked.dispose();
      disposedModels.add(checked);
      disposedModels.add(Number(model));
    },
    tfjs_dispose_layers_model: (model) => {
      const checked = asLayersModel(model);
      if (typeof checked.dispose !== 'function') {
        throw new TypeError('TensorFlow.js LayersModel does not support dispose()');
      }
      checked.dispose();
      disposedModels.add(checked);
      disposedModels.add(Number(model));
    },
    tfjs_graph_model_predict: (model, input) =>
      rememberTensor(asSingleOutputTensor(asGraphModel(model).predict(asTensor(input)), 'GraphModel.predict')),
    tfjs_graph_model_predict_async: async (model, input) =>
      rememberTensor(asSingleOutputTensor(await asGraphModel(model).predictAsync(asTensor(input)), 'GraphModel.predictAsync')),
    tfjs_graph_model_execute: (model, input) =>
      rememberTensor(asSingleOutputTensor(asGraphModel(model).execute(asTensor(input)), 'GraphModel.execute')),
    tfjs_layers_model_predict: (model, input) =>
      rememberTensor(asSingleOutputTensor(asLayersModel(model).predict(asTensor(input)), 'LayersModel.predict')),
    tfjs_layers_model_compile_sgd: (model, loss, learningRate) => {
      const tfjs = ensureTfjs();
      const checked = asLayersModel(model);
      if (typeof checked.compile !== 'function') {
        throw new TypeError('TensorFlow.js LayersModel does not support compile()');
      }
      checked.compile({
        optimizer: tfjs.train.sgd(checkPositiveFinite(learningRate, 'learning_rate')),
        loss: assertBuiltinLossName(tfjs, loss),
      });
    },
    tfjs_layers_model_fit_one: async (model, x, y, epochs, batchSize) => {
      const checked = asLayersModel(model);
      if (typeof checked.fit !== 'function') {
        throw new TypeError('TensorFlow.js LayersModel does not support fit()');
      }
      return checked.fit(asTensor(x), asTensor(y), {
        epochs: checkPositiveInteger(epochs, 'epochs'),
        batchSize: checkPositiveInteger(batchSize, 'batch_size'),
        shuffle: false,
        verbose: 0,
        yieldEvery: 'auto',
      }).then(rememberTrainingHistory);
    },
    tfjs_training_history_len: (history) => lossHistory(history).length,
    tfjs_training_history_loss: historyLossAt,
    tfjs_graph_model_input_count: (model) => modelCount(asGraphModel(model), 'inputs', 'GraphModel'),
    tfjs_graph_model_output_count: (model) => modelCount(asGraphModel(model), 'outputs', 'GraphModel'),
    tfjs_layers_model_input_count: (model) => modelCount(asLayersModel(model), 'inputs', 'LayersModel'),
    tfjs_layers_model_output_count: (model) => modelCount(asLayersModel(model), 'outputs', 'LayersModel'),
    'Tensor.__add': (left, right) => rememberTensor(ensureTfjs().add(asTensor(left), asTensor(right))),
    'Tensor.__sub': (left, right) => rememberTensor(ensureTfjs().sub(asTensor(left), asTensor(right))),
    'Tensor.__mul': (left, right) => rememberTensor(ensureTfjs().mul(asTensor(left), asTensor(right))),
    'Tensor.__div': (left, right) => rememberTensor(ensureTfjs().div(asTensor(left), asTensor(right))),
    'Tensor.__neg': (tensor) => rememberTensor(ensureTfjs().neg(asTensor(tensor))),
  };
}

export function buildWaluauImports(wasmModule, initLogger, options = {}) {
  // Compiler metadata is authoritative for new artifacts. Reflection, binary
  // import decoding, and the byte-constant custom section remain as a
  // compatibility fallback for older callers/artifacts during migration.
  const wasmImports = options.requiredImports ?? getWasmImports(wasmModule, options.wasmBytes);
  const bytesConstants = options.bytesConstants
    ? Array.from(options.bytesConstants, (value) => new Uint8Array(value))
    : decodeBytesConstantsFromWasm(wasmModule);
  // Modules that use linear-memory typed arrays import their memory so host
  // functions can view it even while the start function runs (before the
  // instance and its exports exist).
  const needsMemory = wasmImports.some(
    (wasmImport) =>
      wasmImport.module === WALUAU_IMPORT_MODULE &&
      wasmImport.kind === 'memory' &&
      wasmImport.name === 'memory'
  );
  const wasmMemory = needsMemory ? new WebAssembly.Memory({ initial: 1 }) : null;
  const getWasmMemory = () => wasmMemory;
  const domHost = createPlaygroundDomHost(
    wasmImports,
    options.domOutputRoot,
    options.getWasmExports,
    getWasmMemory
  );
  const tfjsHost = createTfjsHost(options.getWasmExports);
  const gameServicesHost = createGameServicesHost({
    document: options.domOutputRoot,
    ...(options.gameServices ?? {}),
  });
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
  // math.random state (mulberry32). Seeded randomly per instance so
  // unseeded runs differ; math.randomseed(x) resets it deterministically.
  let prngState = (Math.random() * 0x100000000) >>> 0;
  const externIs = (value, typeName) => {
    const name = String(typeName);
    // Nodes carry their realm via ownerDocument; events (which have no
    // ownerDocument) come from the DOM Output iframe's realm, so resolve the
    // constructor through the event target's document instead -- an
    // 'instanceof globalThis.KeyboardEvent' check would always be false for
    // an event created inside the iframe.
    const view =
      value?.ownerDocument?.defaultView ??
      (value?.nodeType === 9 ? value.defaultView : null) ??
      value?.target?.ownerDocument?.defaultView ??
      (value?.target?.nodeType === 9 ? value.target.defaultView : null) ??
      globalThis;
    const ctor = view?.[name] ?? globalThis[name];
    return typeof ctor === 'function' && value instanceof ctor ? 1 : 0;
  };
  const waluauImports = {
    ...domHost,
    ...tfjsHost,
    ...gameServicesHost,
    ...hostImports,
    __waluau_attach_promise: (threadHandle, promise) => {
      if (promise == null || typeof promise.then !== 'function') {
        throw new TypeError('coroutine.await_promise expects a Promise-like extern value');
      }
      // Resolve exports lazily inside invoke because the promise may settle
      // after the explicit main entry point returns.
      const invoke = (payload, rejected) => {
        const exports = options.getWasmExports?.();
        const resume = exports?.[PROMISE_RESUME_TRAMPOLINE_EXPORT];
        const resetActive = exports?.[PROMISE_RESET_ACTIVE_EXPORT];
        if (typeof resume !== 'function') {
          throw new Error(`Missing ${PROMISE_RESUME_TRAMPOLINE_EXPORT} export for Promise await`);
        }
        if (typeof resetActive !== 'function') {
          throw new Error(`Missing ${PROMISE_RESET_ACTIVE_EXPORT} export for Promise await`);
        }
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
    ...createLuaPatternHost(),
    string_len: (value) => String(value).length,
    string_sub: (value, first, last) => {
      // Lua string.sub semantics: 1-based inclusive indices, negatives count
      // from the end (-1 is the last character, which also serves as the
      // compiler's default for a missing `last`).
      const str = String(value);
      const len = str.length;
      const posrelat = (pos) => (pos >= 0 ? pos : Math.max(0, len + pos + 1));
      let start = posrelat(Number(first) | 0);
      let end = posrelat(Number(last) | 0);
      if (start < 1) start = 1;
      if (end > len) end = len;
      return start <= end ? str.slice(start - 1, end) : '';
    },
    string_rep: (value, count, separator) => {
      const n = Math.max(0, Number(count) | 0);
      return Array(n).fill(String(value)).join(String(separator));
    },
    string_byte: (value, index) => {
      // Lua string.byte semantics: 1-based index, negatives count from the
      // end. Out-of-range returns -1 (Lua returns no values; a nilable
      // multi-return string.byte is tracked separately).
      const str = String(value);
      const len = str.length;
      let offset = Number(index) | 0;
      if (offset < 0) offset = len + offset + 1;
      if (offset < 1 || offset > len) return -1;
      return str.codePointAt(offset - 1);
    },
    string_upper: (value) => String(value).toUpperCase(),
    string_lower: (value) => String(value).toLowerCase(),
    string_reverse: (value) => Array.from(String(value)).reverse().join(''),
    ...Object.fromEntries(Array.from({ length: 17 }, (_, arity) => [
      `string_char${arity}`,
      (...args) => stringChar(...args),
    ])),
    ...Object.fromEntries(Array.from({ length: 101 }, (_, arity) => [
      `string_format${arity}`,
      (format, ...args) => stringFormat(format, ...args),
    ])),
    extern_is: externIs,
    // Fallback equality for `unknown` values: the wasm side already unboxes
    // and compares numbers/booleans, so `===` here decides strings (content),
    // nulls, and host/GC objects (identity).
    js_eq_unknown: (left, right) => (left === right ? 1 : 0),
    math_pow: (base, exponent) => Math.pow(base, exponent),
    // math.* builtins (builtins/math.walu). f32/f64 overload pairs share one
    // JS implementation: wasm's import boundary narrows f32 results, and the
    // JS Math functions match wasm float semantics (NaN propagation,
    // Math.min(-0, +0) === -0) for both widths.
    'math.abs': Math.abs,
    'math.sqrt': Math.sqrt,
    'math.floor': Math.floor,
    'math.ceil': Math.ceil,
    'math.trunc': Math.trunc,
    // Round-to-nearest, ties to even (wasm fN.nearest); Math.round rounds
    // half-up so it cannot be used directly.
    'math.nearest': (x) => {
      const floor = Math.floor(x);
      const diff = x - floor;
      if (diff < 0.5) return floor;
      if (diff > 0.5) return floor + 1;
      return floor % 2 === 0 ? floor : floor + 1;
    },
    'math.cos': Math.cos,
    'math.sin': Math.sin,
    'math.min': Math.min,
    'math.max': Math.max,
    'math.copysign': (magnitude, sign) => {
      const negative = sign < 0 || Object.is(sign, -0);
      return negative ? -Math.abs(magnitude) : Math.abs(magnitude);
    },
    // Lua-style math.random / math.randomseed. The generator (mulberry32) is
    // host-owned, per-instance state: unseeded runs start from a random
    // seed, and math.randomseed(x) makes the following sequence fully
    // deterministic (fixtures/snake/sim.walu relies on this).
    //   math.random()     -> f64 in [0, 1)
    //   math.random(m)    -> i32 in [1, m]
    //   math.random(m, n) -> i32 in [m, n]
    'math.random': (m, n) => {
      prngState = (prngState + 0x6d2b79f5) >>> 0;
      let t = prngState;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      const sample = ((t ^ (t >>> 14)) >>> 0) / 4294967296;
      if (m === undefined) return sample;
      const low = n === undefined ? 1 : m | 0;
      const high = n === undefined ? m | 0 : n | 0;
      if (high < low) {
        throw new Error(`math.random interval is empty: [${low}, ${high}]`);
      }
      return low + Math.floor(sample * (high - low + 1));
    },
    'math.randomseed': (seed) => {
      prngState = Math.trunc(Number(seed)) >>> 0;
    },
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
  for (const wasmImport of wasmImports) {
    if (
      wasmImport.module === WALUAU_IMPORT_MODULE &&
      wasmImport.kind === 'function' &&
      (wasmImport.name.startsWith('js_tostring_') ||
        wasmImport.name === 'js_typeof_unknown' ||
        wasmImport.name === 'js_tonumber_string' ||
        wasmImport.name === 'js_tonumber_unknown')
    ) {
      if (wasmImport.name === 'js_typeof_unknown') {
        waluauImports[wasmImport.name] = luauTypeName;
      } else if (
        wasmImport.name === 'js_tonumber_string' ||
        wasmImport.name === 'js_tonumber_unknown'
      ) {
        waluauImports[wasmImport.name] = luauToNumber;
      } else if (wasmImport.name === 'js_tostring_bool') {
        waluauImports[wasmImport.name] = (value) => (value ? 'true' : 'false');
      } else {
        waluauImports[wasmImport.name] = luauToString;
      }
    }
  }
  for (const wasmImport of wasmImports) {
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
  if (wasmMemory) {
    waluauImports.memory = wasmMemory;
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
    case 'Nullable': return `${renderType(type.value.innerType)}?`;
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
    if (type.kind === 'Nullable') return null;
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
    case 'Nullable': {
      if (val === null || val === undefined || val === 'nil') return null;
      return constructArg(val, type.value.innerType, instance, plainTagIds);
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
    case 'Nullable': return inspectVal(val, type.value.innerType, instance, plainTagIds);
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

export function classifyWasmModuleError(err, phase, requiresWasmGc) {
  const message = err?.message || String(err);
  const errorName = err?.name && err.name !== 'Error' ? err.name : '';
  const detail = errorName && !message.startsWith(errorName)
    ? `${errorName}: ${message}`
    : message;
  const isCompileError =
    (typeof WebAssembly !== 'undefined' &&
      (err instanceof WebAssembly.CompileError ||
       err instanceof WebAssembly.LinkError)) ||
    err?.name === 'CompileError' ||
    err?.name === 'LinkError';

  const phaseLabels = {
    compile: 'compile the generated WASM module',
    inspect: 'inspect the generated WASM module',
    imports: 'prepare imports for the generated WASM module',
    instantiate: 'instantiate the generated WASM module',
    execute: 'execute the generated WASM module entry point',
  };
  const lines = [`Failed to ${phaseLabels[phase] || 'load the generated WASM module'}: ${detail}`];

  if (requiresWasmGc && isCompileError) {
    lines.push(
      'This module uses Wasm GC. The browser rejected it during compilation or linking; the exact error above may identify the unsupported instruction or import.',
    );
  }
  return lines.join('\n');
}
