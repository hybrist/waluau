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
  // matchers know to map them back to true/false; enum values arrive as
  // their i32 ordinals). The `not` property getters toggle `negated`
  // instead of chaining vitest's `.not` immediately, so each matcher
  // resolves the final polarity in one step — `flip` folds in matchers
  // that are themselves negated (notToBe), keeping `:not:notToBe` a
  // double negation.
  const wrapValue = (raw) => ({ raw, negated: false });
  const negate = (wrapper) => ({ raw: wrapper.raw, negated: !wrapper.negated });
  const withPolarity = (expectation, wrapper, flip) =>
    Boolean(wrapper.negated) !== Boolean(flip) ? expectation.not : expectation;
  const number = (wrapper, flip) => withPolarity(api.expect(wrapper.raw), wrapper, flip);
  const bool = (wrapper, flip) => withPolarity(api.expect(wrapper.raw !== 0), wrapper, flip);
  const value = (wrapper, flip) => withPolarity(api.expect(wrapper.raw), wrapper, flip);
  // Nullable values arrive as null or the plain payload (the runtime unboxes
  // nullable-primitive box refs before the import runs). i64?/u64? payloads
  // unbox to BigInt; map them to numbers so `toBe` compares against the f64
  // expected value. Nullable bools already arrive as true/false.
  const nilable = (wrapper, flip) => {
    const raw = wrapper.raw === undefined ? null : wrapper.raw;
    return withPolarity(api.expect(typeof raw === 'bigint' ? Number(raw) : raw), wrapper, flip);
  };

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
    // `expect` overloads whose parameter is a nullable primitive import
    // under signature-derived host names (one wasm import name per
    // signature); the runtime unboxes their nullable box-ref argument to a
    // plain value or null before these run, keyed off that unique name.
    'expect#f64?': wrapValue,
    'expect#f32?': wrapValue,
    'expect#i32?': wrapValue,
    'expect#u32?': wrapValue,
    'expect#i64?': wrapValue,
    'expect#u64?': wrapValue,
    'expect#bool?': wrapValue,
    'expect#enum?': wrapValue,

    'NumberExpectation.get/not': negate,
    'NumberExpectation.toBe': (w, expected) => number(w).toBe(expected),
    'NumberExpectation.notToBe': (w, expected) => number(w, true).toBe(expected),
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

    'StringExpectation.get/not': negate,
    'StringExpectation.toBe': (w, expected) => value(w).toBe(expected),
    'StringExpectation.notToBe': (w, expected) => value(w, true).toBe(expected),
    'StringExpectation.toContain': (w, expected) => value(w).toContain(expected),
    'StringExpectation.notToContain': (w, expected) => value(w, true).toContain(expected),
    'StringExpectation.toHaveLength': (w, expected) => value(w).toHaveLength(expected),

    'BoolExpectation.get/not': negate,
    'BoolExpectation.toBe': (w, expected) => bool(w).toBe(expected !== 0),
    'BoolExpectation.toBeTruthy': (w) => bool(w).toBeTruthy(),
    'BoolExpectation.toBeFalsy': (w) => bool(w).toBeFalsy(),

    'ExternExpectation.get/not': negate,
    'ExternExpectation.toBe': (w, expected) => value(w).toBe(expected),
    'ExternExpectation.notToBe': (w, expected) => value(w, true).toBe(expected),

    // Enum values cross the boundary as their i32 ordinals.
    'EnumExpectation.get/not': negate,
    'EnumExpectation.toBe': (w, expected) => value(w).toBe(expected),
    'EnumExpectation.notToBe': (w, expected) => value(w, true).toBe(expected),

    'NullableNumberExpectation.get/not': negate,
    'NullableNumberExpectation.toBe': (w, expected) => nilable(w).toBe(expected),
    'NullableNumberExpectation.notToBe': (w, expected) => nilable(w, true).toBe(expected),
    'NullableNumberExpectation.toBeNil': (w) => nilable(w).toBeNull(),
    'NullableNumberExpectation.notToBeNil': (w) => nilable(w, true).toBeNull(),

    'NullableStringExpectation.get/not': negate,
    'NullableStringExpectation.toBe': (w, expected) => nilable(w).toBe(expected),
    'NullableStringExpectation.notToBe': (w, expected) => nilable(w, true).toBe(expected),
    'NullableStringExpectation.toBeNil': (w) => nilable(w).toBeNull(),
    'NullableStringExpectation.notToBeNil': (w) => nilable(w, true).toBeNull(),

    'NullableBoolExpectation.get/not': negate,
    'NullableBoolExpectation.toBe': (w, expected) => nilable(w).toBe(expected !== 0),
    'NullableBoolExpectation.toBeNil': (w) => nilable(w).toBeNull(),
    'NullableBoolExpectation.notToBeNil': (w) => nilable(w, true).toBeNull(),

    // Nullable enum payloads are i32 ordinals, like EnumExpectation.
    'NullableEnumExpectation.get/not': negate,
    'NullableEnumExpectation.toBe': (w, expected) => nilable(w).toBe(expected),
    'NullableEnumExpectation.notToBe': (w, expected) => nilable(w, true).toBe(expected),
    'NullableEnumExpectation.toBeNil': (w) => nilable(w).toBeNull(),
    'NullableEnumExpectation.notToBeNil': (w) => nilable(w, true).toBeNull(),
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
export async function registerWaluGlueTests({ run, api, wasmUrl, shaderSources }) {
  const host = createWaluTestHost(api);
  try {
    const loaded = await run({
      wasmUrl,
      createImports: (context) => {
        host.setWasmExportsProvider(context.getWasmExports);
        return buildWaluauImports(null, undefined, {
          requiredImports: context.requiredImports,
          bytesConstants: context.bytesConstants,
          hostImports: host.hostImports,
          getWasmExports: context.getWasmExports,
          shaderSources,
          // These suites run in a real browser, so the test document is the
          // DOM output root. A test module only has to reach a DOM extern
          // transitively — requiring a module that itself requires
          // `dom:window` at module scope — for its top level to need one, and
          // failing to mount it turns that into an import error rather than a
          // test result.
          domOutputRoot: typeof document === "undefined" ? undefined : document,
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
