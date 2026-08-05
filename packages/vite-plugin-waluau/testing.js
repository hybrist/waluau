// JS half of the Waluau test bridge: maps the host imports declared in
// externs/vitest.walu (available to test files via require("waluau:vitest"))
// onto a vitest-style API. Suite/test/hook bodies arrive as opaque Waluau
// closure values and are invoked through the module's exported
// __waluau_call_callback_unit trampoline; `expect` values arrive as raw wasm
// boundary values (numbers, JS strings, 0/1 booleans, extern refs) and each
// matcher import knows how to map its receiver back to a JS value.
import * as vitest from 'vitest';
import {
  WALUAU_STRING_CONSTANTS_MODULE,
  WALUAU_MAIN_EXPORT,
  buildWaluauImports,
} from './runtime.js';

const CALLBACK_UNIT_TRAMPOLINE_EXPORT = '__waluau_call_callback_unit';
const LUA_ERROR_TAG_EXPORT = '__waluau_error_tag';

// Rethrows Waluau `error`/`assert` exceptions as plain JS errors carrying the
// Lua error message, so vitest failure output stays readable. Other values
// (vitest expectation errors thrown by our own matcher imports, wasm traps)
// pass through unchanged.
function normalizeWaluError(error, getWasmExports) {
  const tag = getWasmExports()?.[LUA_ERROR_TAG_EXPORT];
  if (
    typeof WebAssembly.Exception === 'function' &&
    error instanceof WebAssembly.Exception &&
    tag instanceof WebAssembly.Tag &&
    error.is(tag)
  ) {
    return new Error(String(error.getArg(tag, 0)));
  }
  return error;
}

// Builds the vitest.walu host imports against a vitest-style API object.
// `api` needs describe/it (with .skip and .todo), beforeEach/afterEach,
// beforeAll/afterAll, and expect — the real vitest module satisfies it, and
// meta-tests can pass a recording fake.
export function createWaluTestHost(api = vitest) {
  let exportsProvider = () => null;
  const getWasmExports = () => exportsProvider();

  const invokeBody = (callback) => {
    const trampoline = getWasmExports()?.[CALLBACK_UNIT_TRAMPOLINE_EXPORT];
    if (typeof trampoline !== 'function') {
      throw new Error(
        `Missing ${CALLBACK_UNIT_TRAMPOLINE_EXPORT} export for walu-test body callback`,
      );
    }
    try {
      trampoline(callback);
    } catch (error) {
      throw normalizeWaluError(error, getWasmExports);
    }
  };
  const asBody = (callback) => () => invokeBody(callback);

  // `expect` wraps the raw boundary value; matcher imports decide how to
  // interpret it (booleans arrive as 0/1 i32s, so only the BoolExpectation
  // matchers know to map them back to true/false).
  const wrapValue = (raw) => ({ raw });
  const number = (wrapper) => api.expect(wrapper.raw);
  const bool = (wrapper) => api.expect(wrapper.raw !== 0);

  const hostImports = {
    describe: (name, body) => api.describe(String(name), asBody(body)),
    xdescribe: (name, body) => api.describe.skip(String(name), asBody(body)),
    it: (name, body) => api.it(String(name), asBody(body)),
    test: (name, body) => api.it(String(name), asBody(body)),
    xit: (name, body) => api.it.skip(String(name), asBody(body)),
    todo: (name) => api.it.todo(String(name)),
    before_each: (body) => api.beforeEach(asBody(body)),
    after_each: (body) => api.afterEach(asBody(body)),
    before_all: (body) => api.beforeAll(asBody(body)),
    after_all: (body) => api.afterAll(asBody(body)),

    expect: wrapValue,

    'NumberExpectation.toBe': (w, expected) => number(w).toBe(expected),
    'NumberExpectation.notToBe': (w, expected) => number(w).not.toBe(expected),
    'NumberExpectation.toBeCloseTo': (w, expected, digits) =>
      digits === undefined
        ? number(w).toBeCloseTo(expected)
        : number(w).toBeCloseTo(expected, digits),
    'NumberExpectation.toBeGreaterThan': (w, expected) => number(w).toBeGreaterThan(expected),
    'NumberExpectation.toBeGreaterThanOrEqual': (w, expected) =>
      number(w).toBeGreaterThanOrEqual(expected),
    'NumberExpectation.toBeLessThan': (w, expected) => number(w).toBeLessThan(expected),
    'NumberExpectation.toBeLessThanOrEqual': (w, expected) =>
      number(w).toBeLessThanOrEqual(expected),

    'StringExpectation.toBe': (w, expected) => api.expect(w.raw).toBe(expected),
    'StringExpectation.notToBe': (w, expected) => api.expect(w.raw).not.toBe(expected),
    'StringExpectation.toContain': (w, expected) => api.expect(w.raw).toContain(expected),
    'StringExpectation.notToContain': (w, expected) => api.expect(w.raw).not.toContain(expected),
    'StringExpectation.toHaveLength': (w, expected) => api.expect(w.raw).toHaveLength(expected),

    'BoolExpectation.toBe': (w, expected) => bool(w).toBe(expected !== 0),
    'BoolExpectation.toBeTruthy': (w) => bool(w).toBeTruthy(),
    'BoolExpectation.toBeFalsy': (w) => bool(w).toBeFalsy(),

    'ExternExpectation.toBe': (w, expected) => api.expect(w.raw).toBe(expected),
    'ExternExpectation.notToBe': (w, expected) => api.expect(w.raw).not.toBe(expected),
  };

  return {
    hostImports,
    setWasmExports: (exports) => {
      exportsProvider = () => exports;
    },
    // The glue-module path resolves exports lazily: suite bodies already run
    // while the glue's run() is still executing (before the caller ever sees
    // the instantiated exports), so the provider must come from the
    // createImports context.
    setWasmExportsProvider: (provider) => {
      exportsProvider = provider;
    },
    getWasmExports,
  };
}

// Registers a test module compiled ahead of time by the waluau CLI, using the
// generated glue module's `run` entry point. Called from the module the
// waluau() vite plugin generates for each *.test.walu file.
export async function registerWaluGlueTests({ run, api }) {
  const host = createWaluTestHost(api);
  try {
    const loaded = await run({
      createImports: (context) => {
        host.setWasmExportsProvider(context.getWasmExports);
        return buildWaluauImports(null, undefined, {
          requiredImports: context.requiredImports,
          bytesConstants: context.bytesConstants,
          hostImports: host.hostImports,
          getWasmExports: context.getWasmExports,
          // These suites run in a real browser, so the test document is the
          // DOM output root. A test module only has to reach a DOM extern
          // transitively — requiring a module that itself requires
          // `dom:window` at module scope — for its top level to need one, and
          // failing to mount it turns that into an import error rather than a
          // test result.
          domOutputRoot: typeof document === 'undefined' ? undefined : document,
        });
      },
    });
    return { exports: loaded.exports, host };
  } catch (error) {
    throw normalizeWaluError(error, host.getWasmExports);
  }
}

// Compiles a *.test.walu source in the browser with the waluau-wasm compiler,
// instantiates it, and runs its top level so the describe/it host imports
// register the suite with vitest. Used by apps that ship the in-browser
// compiler (e.g. the conformance runner).
export async function registerWaluTests({ source, path, init, compile_multi, api }) {
  await init();
  const entryPath = path.startsWith('/') ? path : `/${path}`;
  const output = compile_multi({ [entryPath]: source }, entryPath);
  const wasmBuffer = new Uint8Array(output.wasm);
  const wasmModule = await WebAssembly.compile(wasmBuffer, {
    builtins: ['js-string'],
    importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
  });

  const host = createWaluTestHost(api);
  let instance;
  const imports = buildWaluauImports(wasmModule, undefined, {
    wasmBytes: wasmBuffer,
    requiredImports: output.requiredImports,
    bytesConstants: output.bytesConstants,
    hostImports: host.hostImports,
    getWasmExports: () => instance?.exports,
  });

  instance = await WebAssembly.instantiate(wasmModule, imports);
  host.setWasmExports(instance.exports);
  try {
    instance.exports[WALUAU_MAIN_EXPORT]?.();
  } catch (error) {
    throw normalizeWaluError(error, host.getWasmExports);
  }
  return { exports: instance.exports, host };
}
