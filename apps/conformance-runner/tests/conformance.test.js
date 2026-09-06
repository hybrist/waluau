import { describe, it, expect } from 'vitest';
import {
  compileAndInstantiate,
  compileAndInstantiateWithDom,
  compileAndInstantiateWithExports,
  compileAndRunGeneratedGlue,
} from '../src/runner.js';
import {
  conformanceIncludePaths,
  conformanceExpectations,
  normalizeWhitespace,
  failureMatchesExpected,
} from '../../../tools/conformance/includes.js';
import gameEngineSim from '../../../fixtures/game-engine/sim.walu?raw';
import gameEngineMain from '../../../fixtures/game-engine/main.walu?raw';
import gameEngineTextAlignment from '../../../fixtures/game-engine/text-alignment.walu?raw';
import gameEngineGraphicsPaths from '../../../fixtures/game-engine/graphics-paths.walu?raw';
import gameEngineSessionLifecycle from '../../../fixtures/game-engine/session-lifecycle.walu?raw';
import stableEngineProject from '../../../examples/game-project/main.walu?raw';
import gameEngineGpuShaders from '../../../fixtures/game-engine/gpu-shaders.walu?raw';
import gameEngineShaderSources from '../../../fixtures/game-engine/shader-sources.walu?raw';
import gameEngineGpuResources from '../../../fixtures/game-engine/gpu-resources.walu?raw';
import gameEngineGpuFontResources from '../../../fixtures/game-engine/gpu-font-resources.walu?raw';
const pokerCardBack = '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>';
import pokerFontUrl from '../../../apps/ante/assets/Cinzel-Bold.ttf?url';
import gameEngineBrowser from '../../../engine/browser.walu?raw';
import gameEngineGraphics from '../../../engine/graphics.walu?raw';
import gameEngineFont from '../../../engine/font.walu?raw';
import gameEngineInput from '../../../engine/input.walu?raw';
import gameEngineTime from '../../../engine/time.walu?raw';
import gameEngineResources from '../../../engine/resources.walu?raw';
import gameEngineAudio from '../../../engine/audio.walu?raw';
import gameEngineSave from '../../../engine/save.walu?raw';
import gameEngineShaderSourcesModule from '../../../engine/shader_sources.walu?raw';
import gameEngineResourceSample from '../../../fixtures/game-engine/resources.walu?raw';
import gameEngineParticles from '../../../engine/particles.walu?raw';
import particleSim from '../../../fixtures/particles/sim.walu?raw';
import particleGallery from '../../../fixtures/particles/main.walu?raw';
import particleScenes from '../../../fixtures/particles/scenes.walu?raw';
import { createWaluauShaderSourceHost } from '../../../packages/vite-plugin-waluau/shaders.js';
import transitiveAwaitStateMain from '../../../fixtures/coroutine-await-state/main.walu?raw';
import transitiveAwaitStateWorker from '../../../fixtures/coroutine-await-state/worker.walu?raw';
import luauConformancePreamble from '../src/luau-conformance-preamble.walu?raw';
const conformanceModules = import.meta.glob('../../../conformance/**/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default',
});

const includeModules = import.meta.glob('../../../{builtins,externs}/**/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default',
});

function sourceForCase(testCase) {
  const includes = [];
  if (testCase.name.startsWith('luau/')) {
    includes.push(luauConformancePreamble);
  }
  for (const resolved of conformanceIncludePaths(testCase.name, testCase.source)) {
    const globKey = `../../..${resolved}`;
    const source = includeModules[globKey];
    if (source === undefined) {
      throw new Error(`Unknown conformance include resolved to ${resolved}`);
    }
    includes.push(source);
  }
  if (includes.length === 0) {
    return testCase.source;
  }
  return `${includes.join('\n')}\n${testCase.source}`;
}

function optionsForCase(name) {
  if (name === 'coroutine_await_promise.walu') {
    return createPromiseAwaitHarness().options;
  }
  if (name === 'promise_await.walu') {
    return createTypedPromiseAwaitHarness().options;
  }
  if (name === 'tfjs_model_loading.walu') {
    return createTfjsModelHarness().options;
  }

  if (name !== 'extern_member_collisions.walu') {
    return undefined;
  }

  const alpha = { size: 10 };
  const beta = { size: 20 };

  return {
    hostImports: {
      make_alpha() {
        return alpha;
      },
      make_beta() {
        return beta;
      },
      'Alpha.get/size'(receiver) {
        return receiver.size;
      },
      'Alpha.set/size'(receiver, value) {
        receiver.size = value;
      },
      'Beta.get/size'(receiver) {
        return receiver.size;
      },
      'Beta.set/size'(receiver, value) {
        receiver.size = value;
      },
      'Alpha.value'(receiver, delta) {
        return receiver.size * 9 + delta + 1;
      },
      'Beta.value'(receiver, delta) {
        return receiver.size * 9 + delta + 2;
      },
    },
  };
}

function createPromiseAwaitHarness() {
  const strings = [];
  const objects = [];
  const nested = [];
  const statuses = [];
  const asyncErrors = [];
  const objectValue = { id: 'promise-object' };

  return {
    strings,
    objects,
    nested,
    statuses,
    asyncErrors,
    objectValue,
    options: {
      hostImports: {
        make_string_promise() {
          return Promise.resolve('settled');
        },
        make_object_promise() {
          return Promise.resolve(objectValue);
        },
        make_rejected_promise() {
          return Promise.reject(new Error('promise rejected'));
        },
        record_string(value) {
          strings.push(value);
        },
        record_object(value) {
          objects.push(value);
        },
        record_nested(value) {
          nested.push(value);
        },
        record_status(value) {
          statuses.push(value);
        },
      },
      onAsyncError(error) {
        asyncErrors.push(error);
      },
    },
    async flush() {
      await Promise.resolve();
      await Promise.resolve();
      await new Promise((resolve) => setTimeout(resolve, 0));
    },
  };
}

function createTypedPromiseAwaitHarness() {
  let responseCount = 0;
  const strings = [];
  const responseValue = { ok: true, url: '/test.json' };

  return {
    get responseCount() {
      return responseCount;
    },
    strings,
    responseValue,
    options: {
      hostImports: {
        fetch(url) {
          return Promise.resolve({ ...responseValue, url });
        },
        make_response() {
          return Promise.resolve({ ...responseValue });
        },
        make_text() {
          return Promise.resolve('typed body');
        },
        record_response(_value) {
          responseCount += 1;
        },
        record_string(value) {
          strings.push(value);
        },
      },
    },
    async flush() {
      await Promise.resolve();
      await Promise.resolve();
      await new Promise((resolve) => setTimeout(resolve, 0));
    },
  };
}

function createTfjsModelHarness() {
  const values = [];
  const asyncErrors = [];

  return {
    values,
    asyncErrors,
    options: {
      hostImports: {
        record_tfjs_model_value(value) {
          values.push(value);
        },
        make_training_history_without_loss() {
          return { history: {} };
        },
      },
      onAsyncError(error) {
        asyncErrors.push(error);
      },
    },
  };
}

function tfjsModelHarnessState(harness) {
  return {
    values: harness.values,
    errors: harness.asyncErrors.map(String),
  };
}

const cases = Object.entries(conformanceModules)
  .map(([path, source]) => {
    const normalized = path.replace(/^.*\/conformance\//, '');
    return { name: normalized, source };
  })
  .sort((a, b) => a.name.localeCompare(b.name));

// Intentional Luau VM/JIT stress chunks that are outside Waluau's Wasm-GC
// target. Keep this exact-name set narrow: unlike fixable pending gaps, these
// files should not be compiled by the standard browser suite. native.53's
// enormous register-spill expression takes roughly 90 seconds once dynamic
// numeric inference succeeds, only to reach its irrelevant `is_native` check.
const INTENTIONAL_VM_JIT_EXCLUSIONS = new Set(['luau/native.53.walu']);

// Cases that have a dedicated test below (async DOM entry points that need
// the iframe to stay alive until the async work completes).
const DEDICATED_ASYNC_DOM_CASES = new Set(['top_level_fetch.walu']);

// Runs a case and reports whether it failed (compile/type error, or a trapping
// top-level assert) along with the failure message. Compile errors surface as
// thrown strings from compile_multi; top-level execution traps surface as a
// WebAssembly.RuntimeError, so we read .message when present and fall back to
// String(err) otherwise.
async function runConformanceOutcome(fullSource, options) {
  try {
    await compileAndInstantiate({ '/main.walu': fullSource }, '/main.walu', options);
    return { failed: false, message: null };
  } catch (err) {
    return { failed: true, message: err instanceof Error ? err.message : String(err) };
  }
}

describe('browser conformance', () => {
  it('scopes the Luau native fallback probe to imported conformance chunks', async () => {
    const probe = 'assert(not is_native_if_supported())';
    await expect(
      compileAndInstantiate(
        { '/main.walu': sourceForCase({ name: 'luau/harness-probe.walu', source: probe }) },
        '/main.walu',
      ),
    ).resolves.toBeUndefined();

    const productionOutcome = await runConformanceOutcome(probe);
    expect(productionOutcome.failed).toBe(true);
    expect(productionOutcome.message).toContain("unknown name 'is_native_if_supported'");
  });

  it('evaluates binary literals with Luau separators and existing numeric types', async () => {
    const source = `
      export function binary_default(): f64
        return 0b0000_1000_0001_0000_0100_0010_0010_0101
      end

      export function binary_i32(): i32
        return 0B_0111_1111_1111_1111_1111_1111_1111_1111_
      end

      export function binary_u32(): u32
        return 0b1111_1111_1111_1111_1111_1111_1111_1111
      end
    `;
    const exports = await compileAndInstantiateWithExports({ '/main.walu': source }, '/main.walu');

    expect(exports.binary_default()).toBe(0x08104225);
    expect(exports.binary_i32()).toBe(0x7fffffff);
    expect(exports.binary_u32() >>> 0).toBe(0xffffffff);
  });

  it('checks arithmetic on untyped parameters across both number boxes', async () => {
    const source = `
      local function subtract_one(value)
        return value - 1
      end

      local function negate(value)
        return -value
      end

      local small: i32 = 7
      local precise: number = 2.5
      assert(subtract_one(small) == 6)
      assert(subtract_one(precise) == 1.5)
      assert(negate(small) == -7)
      assert(negate(precise) == -2.5)

      local sub_ok, sub_error = pcall(subtract_one, "bad")
      local neg_ok, neg_error = pcall(negate, false)
      assert(not sub_ok)
      assert(not neg_ok)
      assert(sub_error::string == "attempt to perform arithmetic on a non-number value")
      assert(neg_error::string == "attempt to perform arithmetic on a non-number value")
    `;

    await expect(
      compileAndInstantiate({ '/main.walu': source }, '/main.walu'),
    ).resolves.toBeUndefined();
  });

  for (const { name, source } of cases) {
    if (DEDICATED_ASYNC_DOM_CASES.has(name)) continue;

    const { pending, untriaged, outOfScope, expectedErrors } = conformanceExpectations(source);
    // An out-of-scope chunk is inverse-tested exactly like a pending one: it
    // must still fail today, so a chunk that starts passing breaks the suite
    // whichever marker it carries. They differ only in whether anyone is
    // expected to make them pass. `untriaged` is a variant of pending, so
    // `pending` is already true for it here.
    const inverseTested = pending || outOfScope.length > 0;
    const fullSource = sourceForCase({ name, source });
    const options = optionsForCase(name);

    if (INTENTIONAL_VM_JIT_EXCLUSIONS.has(name)) {
      it.skip(`excluded ${name} (intentional deviation)`, () => {});
    } else if (expectedErrors.length > 0 && inverseTested) {
      // Fail test that is also pending: the expected failure is not produced
      // yet, so verify the actual outcome does NOT match it. When the bug is
      // fixed and the expected failure appears, this test breaks, prompting
      // removal of the `pending` marker.
      it(`pending fail ${name} (not yet producing expected failure)`, async () => {
        const outcome = await runConformanceOutcome(fullSource, options);
        expect(failureMatchesExpected(outcome.message ?? '', expectedErrors)).toBe(false);
      });
    } else if (expectedErrors.length > 0) {
      it(`fails ${name} with expected error`, async () => {
        const outcome = await runConformanceOutcome(fullSource, options);
        expect(outcome.failed).toBe(true);
        for (const fragment of expectedErrors) {
          expect(normalizeWhitespace(outcome.message)).toContain(normalizeWhitespace(fragment));
        }
      });
    } else if (inverseTested) {
      // Inverse test: a pending chunk should pass eventually but does not yet,
      // an untriaged one has an undecided bucket, and an out-of-scope one is
      // not expected to pass at all. In every case we only verify that it
      // currently fails; we do not care how.
      const label = untriaged
        ? `untriaged (${untriaged})`
        : pending
          ? 'pending'
          : `out-of-scope (${outOfScope.join(', ')})`;
      it(`${label} ${name} (currently fails)`, async () => {
        const outcome = await runConformanceOutcome(fullSource, options);
        expect(outcome.failed).toBe(true);
      });
    } else {
      it(`passes ${name}`, async () => {
        await expect(
          compileAndInstantiate({ '/main.walu': fullSource }, '/main.walu', options),
        ).resolves.toBeUndefined();
      });
    }
  }

  it('passes module constants exported across require boundaries', async () => {
    const config = `
      local CELL_SIZE <const>: f64 = 16.0
      local COLS <const>: i32 = 21
      local TITLE <const> = "snake"

      function cell_px(v: i32): f64
          return v::f64 * CELL_SIZE
      end

      return {
          CELL_SIZE = CELL_SIZE,
          COLS = COLS,
          TITLE = TITLE,
          cell_px = cell_px,
      }
    `;
    const main = `
      local config = require("./config")

      assert(config.CELL_SIZE == 16.0)
      assert(config.COLS == 21)
      assert(config.TITLE == "snake")
      assert(config.cell_px(2) == 32.0)

      local width: f64 = config.CELL_SIZE * config.COLS::f64
      assert(width == 336.0)

      function in_function(): f64
          return config.CELL_SIZE + 1.0
      end
      assert(in_function() == 17.0)
    `;
    await expect(
      compileAndInstantiate({ '/config.walu': config, '/main.walu': main }, '/main.walu'),
    ).resolves.toBeUndefined();
  });

  it('calls a dependency local function exported through a legacy trailing return', async () => {
    const helper = `
      local function recurse(depth: i32): i32
          if depth == 0 then
              return 11
          end
          return recurse(depth - 1) + 1
      end

      return recurse
    `;
    const main = `
      local recurse = require("./helper")
      assert(recurse(3) == 14)
    `;
    await expect(
      compileAndInstantiate({ '/helper.walu': helper, '/main.walu': main }, '/main.walu'),
    ).resolves.toBeUndefined();
  });

  it('passes modules imported through single-quoted require paths', async () => {
    const ops = `
      function add(a: i32, b: i32): i32
          return a + b
      end

      return { add = add }
    `;
    const scale = `
      const FACTOR: i32 = 3

      function scale(value: i32): i32
          return value * FACTOR
      end

      return { scale = scale }
    `;
    const main = `
      local ops = require('./ops')
      local scaling = require './scale'

      assert(ops.add(2, 3) == 5)
      assert(scaling.scale(ops.add(2, 5)) == 21)
    `;
    await expect(
      compileAndInstantiate(
        { '/ops.walu': ops, '/scale.walu': scale, '/main.walu': main },
        '/main.walu',
      ),
    ).resolves.toBeUndefined();
  });

  it('assigns a collection of tagged unions into a record field across modules', async () => {
    const shop = `
      type Goods = Upgrade({ kind: i32 }) | Spell({ kind: i32 })
      type Category = "spells"
      type Slot = { category: Category, goods: Goods, price: i32 }
      export type State = { slots: {Slot}, cursor: i32 }

      function new_state(): State
          return { slots = {}, cursor = 0 }
      end

      function roll(): {Slot}
          local slots: {Slot} = {}
          table.insert(slots, { category = "spells", goods = Upgrade({ kind = 1 }), price = 4 })
          table.insert(slots, { category = "spells", goods = Spell({ kind = 2 }), price = 7 })
          return slots
      end

      -- The field's declared type names the union; the value reaching it holds
      -- the canonical record the union is represented by. Across a module
      -- boundary the two namings meet here rather than being normalized together.
      function open_for(state: State): unit
          state.slots = roll()
      end

      function kind_of(slot: Slot): i32
          if Upgrade(upgrade) = slot.goods then return upgrade.kind end
          if Spell(spell) = slot.goods then return 100 + spell.kind end
          return 0
      end

      return {
          new_state = new_state,
          open_for = open_for,
          kind_of = kind_of,
      }
    `;
    const main = `
      local shop = require("./shop")

      local state: shop.State = shop.new_state()
      shop.open_for(state)
      assert(#state.slots == 2)
      assert(shop.kind_of(state.slots[1]) == 1)
      assert(shop.kind_of(state.slots[2]) == 102)
      assert(state.slots[1].goods is Upgrade)
      assert(state.slots[2].price == 7)
    `;
    await expect(
      compileAndInstantiate({ '/shop.walu': shop, '/main.walu': main }, '/main.walu'),
    ).resolves.toBeUndefined();
  });

  it('clones typed aggregate constants at every module use', async () => {
    const defaults = `
      type Inner = { value: i32 }
      type Defaults = { inner: Inner, values: {i32} }

      const BASE: i32 = 7
      const DEFAULTS: Defaults = {
          inner = { value = BASE },
          values = { BASE, 8::i32 },
      }

      local changed = DEFAULTS
      changed.inner.value = 55
      changed.values[1] = 55
      local unchanged = DEFAULTS
      assert(unchanged.inner.value == BASE)
      assert(unchanged.values[1] == BASE)

      function defaults_are_independent(): bool
          local first: Defaults = DEFAULTS
          first.inner.value = 99
          first.values[1] = 99
          local second: Defaults = DEFAULTS
          return second.inner.value == BASE and second.values[1] == BASE
      end

      return {
          DEFAULTS = DEFAULTS,
          defaults_are_independent = defaults_are_independent,
      }
    `;
    const main = `
      local defaults = require("./defaults")

      assert(defaults.defaults_are_independent())
      local first = defaults.DEFAULTS
      first.inner.value = 101
      first.values[1] = 101
      local second = defaults.DEFAULTS
      assert(second.inner.value == 7)
      assert(second.values[1] == 7)
    `;

    await expect(
      compileAndInstantiate({ '/defaults.walu': defaults, '/main.walu': main }, '/main.walu'),
    ).resolves.toBeUndefined();
  });

  it('keeps record module locals used by public functions readable and assignable', async () => {
    const targets = `
      type Box = { x: f64 }
      type Targets = { active: Box }

      function nowhere(): Box
          return { x = -1.0 }
      end

      local targets: Targets = { active = nowhere() }

      function set_target(x: f64): unit
          targets = { active = { x = x } }
      end

      function target_x(): f64
          return targets.active.x
      end

      return { set_target = set_target, target_x = target_x }
    `;
    const main = `
      local targets = require("./targets")
      assert(targets.target_x() == -1.0)
      targets.set_target(41.0)
      assert(targets.target_x() == 41.0)
    `;

    await expect(
      compileAndInstantiate({ '/targets.walu': targets, '/main.walu': main }, '/main.walu'),
    ).resolves.toBeUndefined();
  });

  it('passes opaque records through module operations without exposing fields', async () => {
    const counter = `
      export opaque type Counter = { value: i32 }
      local shared: Counter = { value = 10::i32 }

      function new(value: i32): Counter
          return { value = value }
      end

      function Counter:add(delta: i32): unit
          self.value += delta
      end

      function Counter:value(): i32
          return self.value
      end

      function shared_counter(): Counter
          return shared
      end

      return { new = new, shared_counter = shared_counter }
    `;
    const main = `
      local counters = require("./counter")

      local first: counters.Counter = counters.new(40)
      first:add(2)
      assert(first:value() == 42)

      local shared: counters.Counter = counters.shared_counter()
      shared:add(5)
      assert(shared:value() == 15)

      function pass_through(value: counters.Counter): counters.Counter
          return value
      end

      assert(pass_through(first):value() == 42)
    `;

    await expect(
      compileAndInstantiate({ '/counter.walu': counter, '/main.walu': main }, '/main.walu'),
    ).resolves.toBeUndefined();
  });

  it('passes type aliases imported across require boundaries', async () => {
    const state = `
      export type State = { score: i32 }

      function new_state(): State
          return { score = 41::i32 }
      end

      return {
          new_state = new_state,
      }
    `;
    const main = `
      local state_mod = require("./state")

      type State = state_mod.State

      function bump(state: State): i32
          state.score += 1
          return state.score
      end

      function direct(state: state_mod.State): i32
          return state.score
      end

      local state: state_mod.State = state_mod.new_state()
      assert(bump(state) == 42)
      assert(direct(state) == 42)
    `;
    await expect(
      compileAndInstantiate({ '/state.walu': state, '/main.walu': main }, '/main.walu'),
    ).resolves.toBeUndefined();
  });

  it('passes methods and static constructors across require boundaries', async () => {
    const counter = `
      type Counter = { value: i32 }

      function Counter.new(start: i32): Counter
          local counter: Counter = { value = start }
          counter:clamp()
          return counter
      end

      function Counter:bump(amount: i32): unit
          self.value += amount
          self:clamp()
      end

      function Counter:clamp(): unit
          if self.value > 100 then
              self.value = 100
          end
      end

      return { new = Counter.new }
    `;
    const main = `
      local counter = require("./counter")

      local c = counter.new(5)
      c:bump(10)
      assert(c.value == 15)
      c:bump(1000)
      assert(c.value == 100)

      -- A consumer-side structurally identical alias still dispatches the
      -- defining module's methods.
      type Counter = { value: i32 }
      local aliased: Counter = counter.new(7)
      aliased:bump(1)
      assert(aliased.value == 8)
    `;
    await expect(
      compileAndInstantiate({ '/counter.walu': counter, '/main.walu': main }, '/main.walu'),
    ).resolves.toBeUndefined();
  });

  it('passes extern_host_object.walu round-trip identity checks', async () => {
    const source = cases.find(({ name }) => name === 'extern_host_object.walu').source;
    const exports = await compileAndInstantiateWithExports({ '/main.walu': source }, '/main.walu');
    const element = { id: 'root' };

    expect(exports.identity(element)).toBe(element);
    expect(exports.pass_back(exports.identity(element))).toBe(element);
  });

  it('passes nullable_extern_host_object.walu null and non-null paths', async () => {
    const source = cases.find(({ name }) => name === 'nullable_extern_host_object.walu').source;
    const exports = await compileAndInstantiateWithExports({ '/main.walu': source }, '/main.walu');
    const element = { id: 'root' };

    expect(exports.nullable_score(null)).toBe(10);
    expect(exports.nullable_score(element)).toBe(20);
    expect(exports.nullable_eq_score(null)).toBe(30);
    expect(exports.nullable_eq_score(element)).toBe(20);
  });

  it('passes callback_host_import.walu through the exported event trampoline', async () => {
    const source = cases.find(({ name }) => name === 'callback_host_import.walu').source;
    let callback;
    const reported = [];
    const exports = await compileAndInstantiateWithExports(
      { '/main.walu': source },
      '/main.walu',
      {
        hostImports: {
          register_event_callback(handler) {
            callback = handler;
          },
          report_event_count(value) {
            reported.push(value);
          },
        },
      },
    );

    expect(typeof exports.__waluau_call_callback_event_unit).toBe('function');
    exports.register_counter(41);
    expect(callback).toBeDefined();
    exports.__waluau_call_callback_event_unit(callback, { type: 'click' });
    exports.__waluau_call_callback_event_unit(callback, { type: 'click' });
    expect(reported).toEqual([42, 43]);
  });

  it('passes callback_unit_host_import.walu through the exported unit trampoline', async () => {
    const source = cases.find(({ name }) => name === 'callback_unit_host_import.walu').source;
    let callback;
    const reported = [];
    const exports = await compileAndInstantiateWithExports(
      { '/main.walu': source },
      '/main.walu',
      {
        hostImports: {
          register_body_callback(body) {
            callback = body;
          },
          report_run_count(value) {
            reported.push(value);
          },
        },
      },
    );

    expect(typeof exports.__waluau_call_callback_unit).toBe('function');
    exports.register_runner(6);
    expect(callback).toBeDefined();
    exports.__waluau_call_callback_unit(callback);
    exports.__waluau_call_callback_unit(callback);
    expect(reported).toEqual([7, 8]);
  });

  it('passes callback_f64_host_import.walu through the exported frame trampoline', async () => {
    const source = cases.find(({ name }) => name === 'callback_f64_host_import.walu').source;
    let callback;
    const reported = [];
    const exports = await compileAndInstantiateWithExports(
      { '/main.walu': source },
      '/main.walu',
      {
        hostImports: {
          register_frame_callback(handler) {
            callback = handler;
          },
          report_frame_total(value) {
            reported.push(value);
          },
        },
      },
    );

    expect(typeof exports.__waluau_call_callback_f64_unit).toBe('function');
    exports.register_accumulator(1.5);
    expect(callback).toBeDefined();
    exports.__waluau_call_callback_f64_unit(callback, 16.25);
    exports.__waluau_call_callback_f64_unit(callback, 33.5);
    expect(reported).toEqual([17.75, 51.25]);
  });

  it('passes coroutine_await_promise.walu fulfillment, rejection, and nested resume checks', async () => {
    const source = cases.find(({ name }) => name === 'coroutine_await_promise.walu').source;
    const harness = createPromiseAwaitHarness();
    const exports = await compileAndInstantiateWithExports(
      { '/main.walu': source },
      '/main.walu',
      harness.options,
    );

    exports.run_string();
    exports.run_object();
    exports.run_nested();
    exports.run_recursive_await();
    exports.run_rejected();
    await harness.flush();

    expect(harness.strings).toEqual(['settled']);
    expect(harness.objects).toEqual([harness.objectValue]);
    expect(harness.nested).toEqual(['settled:inner-yield:17', 'rec:base:settled1:settled2:settled3']);
    expect(harness.statuses).toEqual([]);
    expect(harness.asyncErrors).toHaveLength(1);
    expect(String(harness.asyncErrors[0])).toContain('unreachable');
  });

  it('passes promise_await.walu typed function and method forms', async () => {
    const source = cases.find(({ name }) => name === 'promise_await.walu').source;
    const harness = createTypedPromiseAwaitHarness();
    const exports = await compileAndInstantiateWithExports(
      { '/main.walu': source },
      '/main.walu',
      harness.options,
    );

    exports.run_function_form();
    exports.run_method_form();
    await harness.flush();

    expect(harness.responseCount).toBe(2);
    expect(harness.strings).toEqual(['typed body', 'typed body']);
  });

  it('preserves transitive await progress and concrete record locals', async () => {
    const steps = [];
    const asyncErrors = [];
    await compileAndInstantiateWithExports(
      {
        '/main.walu': transitiveAwaitStateMain,
        '/worker.walu': transitiveAwaitStateWorker,
      },
      '/main.walu',
      {
        hostImports: {
          make_state_promise(value) {
            return Promise.resolve(value + 100);
          },
          record_state_step(value) {
            steps.push(value);
          },
        },
        onAsyncError(error) {
          asyncErrors.push(error);
        },
      },
    );

    for (let index = 0; index < 8; index += 1) {
      await Promise.resolve();
    }
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(steps).toEqual([1, 2, 3, 367]);
    expect(asyncErrors).toEqual([]);
  });

  it('passes tfjs_host_api.walu async data readback checks', async () => {
    const source = cases.find(({ name }) => name === 'tfjs_host_api.walu').source;
    const values = [];
    const exports = await compileAndInstantiateWithExports(
      { '/main.walu': sourceForCase({ name: 'tfjs_host_api.walu', source }) },
      '/main.walu',
      {
        hostImports: {
          record_tfjs_async(value) {
            values.push(value);
          },
        },
      },
    );

    exports.run_async_readback();
    await Promise.resolve();
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(values).toEqual([13]);
  });

  it('passes tfjs_host_api.walu tidy and keep lifetime checks', async () => {
    const source = cases.find(({ name }) => name === 'tfjs_host_api.walu').source;
    const exports = await compileAndInstantiateWithExports(
      { '/main.walu': sourceForCase({ name: 'tfjs_host_api.walu', source }) },
      '/main.walu',
    );

    exports.run_lifetime_checks();
  });

  it('passes tfjs_model_loading.walu graph/layers fixtures and multi-output errors', async () => {
    const source = cases.find(({ name }) => name === 'tfjs_model_loading.walu').source;
    const harness = createTfjsModelHarness();
    const exports = await compileAndInstantiateWithExports(
      { '/main.walu': sourceForCase({ name: 'tfjs_model_loading.walu', source }) },
      '/main.walu',
      harness.options,
    );

    exports.run_layers_model_fixture();
    await expect.poll(() => tfjsModelHarnessState(harness)).toEqual({
      values: [9],
      errors: [],
    });

    exports.run_graph_model_fixture();
    await expect.poll(() => tfjsModelHarnessState(harness)).toEqual({
      values: [9, 7],
      errors: [],
    });

    exports.run_graph_multi_output_error();
    await expect.poll(() => harness.asyncErrors.map(String)).toEqual([
      expect.stringContaining('GraphModel.execute returned multiple outputs'),
    ]);

    exports.run_graph_named_output_error();
    await expect.poll(() => harness.asyncErrors.map(String)).toEqual([
      expect.stringContaining('GraphModel.execute returned multiple outputs'),
      expect.stringContaining('GraphModel.predict returned a named output map'),
    ]);
  });

  it('passes tfjs_model_loading.walu layers training fixture', async () => {
    const source = cases.find(({ name }) => name === 'tfjs_model_loading.walu').source;
    const harness = createTfjsModelHarness();
    const exports = await compileAndInstantiateWithExports(
      { '/main.walu': sourceForCase({ name: 'tfjs_model_loading.walu', source }) },
      '/main.walu',
      harness.options,
    );

    exports.run_layers_model_training_fixture();
    await expect.poll(() => tfjsModelHarnessState(harness)).toEqual({
      values: [expect.any(Number)],
      errors: [],
    });

    expect(() => exports.run_training_history_missing_loss_error()).toThrow(
      'TrainingHistory is missing numeric loss history',
    );
  });

  it('passes top_level_fetch.walu with async main-entry fetch and DOM write', async () => {
    const testCase = cases.find(({ name }) => name === 'top_level_fetch.walu');
    const source = sourceForCase(testCase);
    const { root, cleanup } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');

    try {
      // The Wasm main entry point launches the coroutine and returns; the fetch
      // and DOM write happen asynchronously.  Poll until the body is updated.
      await expect.poll(
        () => root.textContent?.trim(),
        { timeout: 10_000 },
      ).toMatch(/fetch body from conformance/);
    } finally {
      cleanup();
    }
  });

  it('passes generated DOM fetch and Response.text await flow', async () => {
    const testCase = cases.find(({ name }) => name === 'dom_fetch_response_text.walu');
    const source = sourceForCase(testCase);
    const { exports, root, cleanup } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');

    try {
      exports.run_fetch_response_text();
      await expect.poll(() => root.querySelector('#fetch-body')?.textContent?.trim()).toBe(
        '{"message":"fetch body from conformance"}',
      );
    } finally {
      cleanup();
    }
  });

  it('passes DOM click/input handlers through the exported event trampoline', async () => {
    const testCase = cases.find(({ name }) => name === 'dom_event_callbacks.walu');
    const source = sourceForCase(testCase);
    const { exports, root } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');
    const button = root.children[0];
    const input = root.children[1];
    const status = root.children[2];

    expect(typeof exports.__waluau_call_callback_event_unit).toBe('function');
    expect(button.textContent).toBe('idle');
    expect(status.textContent).toBe('waiting');

    const clickEvent = typeof Event === 'undefined' ? { type: 'click' } : new Event('click', { bubbles: true });
    button.dispatchEvent(clickEvent);
    expect(button.textContent).toBe('clicked once');
    button.dispatchEvent(clickEvent);
    expect(button.textContent).toBe('clicked twice');

    input.value = 'typed card';
    const inputEvent = typeof Event === 'undefined' ? { type: 'input' } : new Event('input', { bubbles: true });
    input.dispatchEvent(inputEvent);

    expect(status.textContent).toBe('input once');
  });

  it('renders DOM extern handles into the conformance DOM root', async () => {
    const testCase = cases.find(({ name }) => name === 'dom_extern_rendering.walu');
    const source = sourceForCase(testCase);
    const { root, cleanup } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');

    try {
      expect(root.children).toHaveLength(2);
      expect(root.children[0].tagName).toBe('H1');
      expect(root.children[0].id).toBe('generated-heading');
      expect(root.children[0].className).toBe('title');
      expect(root.children[0].textContent).toBe('Hello from generated DOM externsleaf');
      expect(root.children[0].children).toHaveLength(1);
      expect(root.children[0].children[0].tagName).toBe('SPAN');
      expect(root.children[0].children[0].className).toBe('leaf');
      expect(root.children[0].children[0].textContent).toBe('leaf');
      expect(root.children[1].tagName).toBe('P');
      expect(root.children[1].id).toBe('generated-paragraph');
      expect(root.children[1].className).toBe('body');
      expect(root.children[1].textContent).toBe('Rendered through generated extern DOM handles');
    } finally {
      cleanup();
    }
  });

  it('renders through generated canvas 2D extern handles', async () => {
    const testCase = cases.find(({ name }) => name === 'dom_canvas_2d_rendering.walu');
    const source = sourceForCase(testCase);
    const { root, cleanup } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');

    try {
      expect(root.children).toHaveLength(1);
      const canvas = root.children[0];
      expect(canvas.tagName).toBe('CANVAS');
      expect(canvas.id).toBe('waluau-canvas-2d');
      expect(canvas.width).toBe(64);
      expect(canvas.height).toBe(32);
      expect(canvas.style.width).toBe('64px');
      expect(canvas.style.height).toBe('32px');
      expect(canvas.getAttribute('data-context-owner')).toBe('true');

      const context = canvas.getContext('2d');
      const pixel = context.getImageData(6, 7, 1, 1).data;
      expect(Array.from(pixel)).toEqual([0, 0, 0, 255]);
    } finally {
      cleanup();
    }
  });

  it('renders through generated WebGL2 extern handles', async () => {
    const testCase = cases.find(({ name }) => name === 'dom_webgl2_rendering.walu');
    const source = sourceForCase(testCase);
    const { root, cleanup } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');

    try {
      expect(root.children).toHaveLength(1);
      const canvas = root.children[0];
      expect(canvas.tagName).toBe('CANVAS');
      expect(canvas.id).toBe('waluau-canvas-webgl2');
      expect(canvas.width).toBe(64);
      expect(canvas.height).toBe(32);

      // The fixture clears to blue and draws a red full-viewport triangle;
      // the host bridge acquires the context with preserveDrawingBuffer, so
      // the drawn frame stays readable here.
      const gl = canvas.getContext('webgl2');
      expect(gl).not.toBeNull();
      const pixel = new Uint8Array(4);
      gl.readPixels(32, 16, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
      expect(Array.from(pixel)).toEqual([255, 0, 0, 255]);
    } finally {
      cleanup();
    }
  });

  it('renders the game-engine fixture through the WebGL2 graphics backend', async () => {
    const { root, cleanup } = await compileAndInstantiateWithDom(
      {
        '/fixtures/game-engine/main.walu': gameEngineMain,
        '/engine/browser.walu': gameEngineBrowser,
        '/engine/graphics.walu': gameEngineGraphics,
        '/engine/resources.walu': gameEngineResources,
        '/engine/font.walu': gameEngineFont,
        '/engine/input.walu': gameEngineInput,
        '/engine/time.walu': gameEngineTime,
      },
      '/fixtures/game-engine/main.walu'
    );

    try {
      const canvas = root.querySelector('#walua-game-canvas');
      expect(canvas).not.toBeNull();
      expect(canvas.width).toBe(320);
      expect(canvas.height).toBe(200);
      const gl = canvas.getContext('webgl2');
      expect(gl).not.toBeNull();

      const readFrame = () => {
        const pixels = new Uint8Array(320 * 200 * 4);
        gl.readPixels(0, 0, 320, 200, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
        return pixels;
      };
      const pixelAt = (pixels, x, y) => {
        // readPixels rows start at the bottom of the canvas.
        const index = ((200 - 1 - y) * 320 + x) * 4;
        return [pixels[index], pixels[index + 1], pixels[index + 2], pixels[index + 3]];
      };

      // Wait until the engine's animation frame presented the scene: the
      // background clear color #0f172a at a point that stays clear of the
      // grid lines, the player square, and both text lines.
      await expect.poll(
        () => pixelAt(readFrame(), 16, 40),
        { timeout: 10_000 },
      ).toEqual([15, 23, 42, 255]);

      // The frame contains the player square color #38bdf8 somewhere.
      const pixels = readFrame();
      let foundPlayer = false;
      for (let i = 0; i < pixels.length; i += 4) {
        if (
          Math.abs(pixels[i] - 56) <= 1 &&
          Math.abs(pixels[i + 1] - 189) <= 1 &&
          Math.abs(pixels[i + 2] - 248) <= 1
        ) {
          foundPlayer = true;
          break;
        }
      }
      expect(foundPlayer).toBe(true);
    } finally {
      cleanup();
    }
  });

  it('builds and launches a standalone project through the stable engine package', async () => {
    const { root, cleanup } = await compileAndInstantiateWithDom(
      { '/main.walu': stableEngineProject },
      '/main.walu'
    );

    try {
      const canvas = root.querySelector('#walua-game-canvas');
      expect(canvas).not.toBeNull();
      expect(canvas.width).toBe(320);
      expect(canvas.height).toBe(180);
      const gl = canvas.getContext('webgl2');
      expect(gl).not.toBeNull();

      await expect.poll(() => {
        const pixel = new Uint8Array(4);
        // The example's cyan rectangle covers this point after the first frame.
        gl.readPixels(32, 180 - 32, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
        return Array.from(pixel);
      }, { timeout: 10_000 }).toEqual([56, 189, 248, 255]);
    } finally {
      cleanup();
    }
  });

  it('retains the canvas and WebGL2 context across suspended Wasm generations', async () => {
    const files = { '/main.walu': gameEngineSessionLifecycle };
    const first = await compileAndInstantiateWithDom(files, '/main.walu');

    try {
      const document = first.root.ownerDocument;
      const initialRoot = first.root.querySelector('main#walua-game');
      const initialCanvas = first.root.querySelector('#walua-game-canvas');
      const initialContext = initialCanvas?.getContext('webgl2');
      expect(initialRoot).not.toBeNull();
      expect(initialCanvas).not.toBeNull();
      expect(initialContext).not.toBeNull();
      await expect.poll(() => first.exports.draw_count(), { timeout: 10_000 }).toBeGreaterThan(0);

      first.exports.suspend_game();
      const suspendedDrawCount = first.exports.draw_count();
      expect(initialRoot.getAttribute('data-waluau-surface-handoff')).toBe('1');

      const replacement = await compileAndInstantiateWithExports(files, '/main.walu', {
        domOutputRoot: document,
      });
      const retainedRoot = first.root.querySelector('main#walua-game');
      const retainedCanvas = first.root.querySelector('#walua-game-canvas');
      expect(retainedRoot).toBe(initialRoot);
      expect(retainedCanvas).toBe(initialCanvas);
      expect(retainedCanvas.getContext('webgl2')).toBe(initialContext);
      expect(retainedRoot.getAttribute('data-waluau-surface-handoff')).toBeNull();
      expect(first.root.querySelectorAll('main#walua-game')).toHaveLength(1);
      expect(first.root.querySelectorAll('#walua-game-canvas')).toHaveLength(1);
      await expect.poll(() => replacement.draw_count(), { timeout: 10_000 }).toBeGreaterThan(0);
      expect(first.exports.draw_count()).toBe(suspendedDrawCount);

      document.dispatchEvent(new document.defaultView.KeyboardEvent('keydown', { key: 'a' }));
      expect(first.exports.keypress_count()).toBe(0);
      expect(replacement.keypress_count()).toBe(1);

      // Once adopted, a stale reference to the suspended Session cannot tear
      // down the surface now owned by its replacement.
      first.exports.stop_game();
      expect(first.root.querySelector('#walua-game-canvas')).toBe(initialCanvas);

      replacement.stop_game();
      expect(first.root.querySelector('#walua-game-canvas')).toBeNull();
    } finally {
      first.cleanup();
    }
  });

  it('runs minimal and feature-rich modules through compiler-generated JavaScript glue', async () => {
    const originalImports = WebAssembly.Module.imports;
    WebAssembly.Module.imports = () => {
      throw new Error('generated glue must not reflect on Wasm imports');
    };
    try {
      const packagedAssets = {
        'assets/message.txt': { url: './assets/message.fingerprint.txt', type: 'text' },
      };
      const minimal = await compileAndRunGeneratedGlue(
        { '/main.walu': 'function answer(): i32\n    return 42\nend' },
        '/main.walu',
        {
          assetBaseUrl: new URL('./packaged/', window.location.href),
          assetManifest: packagedAssets,
        }
      );
      try {
        expect(minimal.exports.answer()).toBe(42);
        expect(Object.keys(minimal.imports)).toEqual([]);
        expect(minimal.assetManifest).toBe(packagedAssets);
        expect(minimal.assetBaseUrl.pathname).toContain('/packaged/');
      } finally {
        minimal.cleanup();
      }

      const game = await compileAndRunGeneratedGlue(
        { '/main.walu': stableEngineProject },
        '/main.walu'
      );
      try {
        expect(Object.keys(game.imports)).toEqual(['waluau']);
        const canvas = game.root.querySelector('#walua-game-canvas');
        expect(canvas).not.toBeNull();
        await expect.poll(() => {
          const gl = canvas.getContext('webgl2');
          const pixel = new Uint8Array(4);
          gl.readPixels(32, 180 - 32, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
          return Array.from(pixel);
        }, { timeout: 10_000 }).toEqual([56, 189, 248, 255]);
      } finally {
        game.cleanup();
      }
    } finally {
      WebAssembly.Module.imports = originalImports;
    }
  });

  it('retains byte-decoding compatibility for artifacts without compiler metadata', async () => {
    const originalImports = WebAssembly.Module.imports;
    WebAssembly.Module.imports = () => {
      throw new Error('legacy compatibility should decode supplied Wasm bytes first');
    };
    try {
      const exports = await compileAndInstantiateWithExports(
        { '/main.walu': 'function literal(): bytes\n    return b"AB"\nend' },
        '/main.walu',
        { compilerMetadata: false }
      );
      expect(Array.from(exports.literal())).toEqual([65, 66]);
    } finally {
      WebAssembly.Module.imports = originalImports;
    }
  });

  it('measures and aligns game-engine text in logical pixels', async () => {
    const { root, cleanup } = await compileAndInstantiateWithDom(
      {
        '/fixtures/game-engine/text-alignment.walu': gameEngineTextAlignment,
        '/engine/browser.walu': gameEngineBrowser,
        '/engine/graphics.walu': gameEngineGraphics,
        '/engine/resources.walu': gameEngineResources,
        '/engine/font.walu': gameEngineFont,
        '/engine/input.walu': gameEngineInput,
        '/engine/time.walu': gameEngineTime,
      },
      '/fixtures/game-engine/text-alignment.walu'
    );

    try {
      const canvas = root.querySelector('#walua-game-canvas');
      expect(canvas).not.toBeNull();
      const gl = canvas.getContext('webgl2');
      expect(gl).not.toBeNull();

      const boundsForColor = (pixels, color) => {
        let minX = 320;
        let maxX = -1;
        let minY = 200;
        let maxY = -1;
        for (let index = 0; index < pixels.length; index += 4) {
          if (
            Math.abs(pixels[index] - color[0]) <= 1 &&
            Math.abs(pixels[index + 1] - color[1]) <= 1 &&
            Math.abs(pixels[index + 2] - color[2]) <= 1
          ) {
            const pixelIndex = index / 4;
            const x = pixelIndex % 320;
            const y = 200 - 1 - Math.floor(pixelIndex / 320);
            minX = Math.min(minX, x);
            maxX = Math.max(maxX, x);
            minY = Math.min(minY, y);
            maxY = Math.max(maxY, y);
          }
        }
        return { minX, maxX, minY, maxY };
      };

      const readFrame = () => {
        const pixels = new Uint8Array(320 * 200 * 4);
        gl.readPixels(0, 0, 320, 200, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
        return pixels;
      };

      await expect.poll(
        () => boundsForColor(readFrame(), [255, 64, 64]),
        { timeout: 10_000 },
      ).toEqual({ minX: 160, maxX: 182, minY: 33, maxY: 39 });

      const pixels = readFrame();
      expect(boundsForColor(pixels, [64, 255, 64])).toEqual(
        { minX: 148, maxX: 170, minY: 73, maxY: 79 },
      );
      expect(boundsForColor(pixels, [64, 128, 255])).toEqual(
        { minX: 136, maxX: 158, minY: 113, maxY: 119 },
      );
    } finally {
      cleanup();
    }
  });

  it('strokes transformed, curved, closed, and oversized game-engine paths', async () => {
    const { root, cleanup } = await compileAndInstantiateWithDom(
      {
        '/fixtures/game-engine/graphics-paths.walu': gameEngineGraphicsPaths,
        '/engine/browser.walu': gameEngineBrowser,
        '/engine/graphics.walu': gameEngineGraphics,
        '/engine/resources.walu': gameEngineResources,
        '/engine/font.walu': gameEngineFont,
        '/engine/input.walu': gameEngineInput,
        '/engine/time.walu': gameEngineTime,
      },
      '/fixtures/game-engine/graphics-paths.walu'
    );

    try {
      const canvas = root.querySelector('#walua-game-canvas');
      expect(canvas).not.toBeNull();
      const gl = canvas.getContext('webgl2');
      expect(gl).not.toBeNull();

      const pixelAt = (pixels, x, y) => {
        const index = ((200 - 1 - y) * 320 + x) * 4;
        return [pixels[index], pixels[index + 1], pixels[index + 2]];
      };
      const hasColorNear = (pixels, x, y, color, radius = 2) => {
        for (let sampleY = y - radius; sampleY <= y + radius; sampleY += 1) {
          for (let sampleX = x - radius; sampleX <= x + radius; sampleX += 1) {
            const sample = pixelAt(pixels, sampleX, sampleY);
            if (sample.every((channel, index) => Math.abs(channel - color[index]) <= 1)) {
              return true;
            }
          }
        }
        return false;
      };
      const readFrame = () => {
        const pixels = new Uint8Array(320 * 200 * 4);
        gl.readPixels(0, 0, 320, 200, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
        return pixels;
      };

      await expect.poll(
        () => hasColorNear(readFrame(), 60, 25, [255, 64, 64]),
        { timeout: 10_000 },
      ).toBe(true);

      const pixels = readFrame();
      // All four sides prove close_path and the deferred non-uniform transform.
      expect(hasColorNear(pixels, 80, 40, [255, 64, 64])).toBe(true);
      expect(hasColorNear(pixels, 60, 55, [255, 64, 64])).toBe(true);
      expect(hasColorNear(pixels, 40, 40, [255, 64, 64])).toBe(true);
      expect(hasColorNear(pixels, 60, 80, [64, 255, 64])).toBe(true);
      expect(hasColorNear(pixels, 180, 75, [64, 128, 255])).toBe(true);
      expect(hasColorNear(pixels, 310, 182, [255, 64, 255])).toBe(true);
    } finally {
      cleanup();
    }
  });

  it('compiles, binds, configures, and releases game-provided shaders', async () => {
    let uniformTypeReads = 0;
    const { root, cleanup } = await compileAndInstantiateWithDom(
      {
        '/fixtures/game-engine/gpu-shaders.walu': gameEngineGpuShaders,
        '/engine/browser.walu': gameEngineBrowser,
        '/engine/graphics.walu': gameEngineGraphics,
        '/engine/resources.walu': gameEngineResources,
        '/engine/font.walu': gameEngineFont,
        '/engine/input.walu': gameEngineInput,
        '/engine/time.walu': gameEngineTime,
      },
      '/fixtures/game-engine/gpu-shaders.walu',
      {
        hostImports: {
          game_gpu_uniform_type(gl, program, name) {
            uniformTypeReads += 1;
            const expected = String(name);
            const count = Number(gl.getProgramParameter(program, gl.ACTIVE_UNIFORMS));
            for (let index = 0; index < count; index += 1) {
              const info = gl.getActiveUniform(program, index);
              if (info?.name === expected) return Number(info.type);
            }
            return 0;
          },
        },
      },
    );
    try {
      const canvas = root.querySelector('#walua-game-canvas');
      expect(canvas).not.toBeNull();
      const gl = canvas.getContext('webgl2');
      const leftPixel = new Uint8Array(4);
      const rightPixel = new Uint8Array(4);
      const secondPixel = new Uint8Array(4);
      await expect.poll(() => {
        gl.readPixels(32, 100 - 1 - 32, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, leftPixel);
        gl.readPixels(96, 100 - 1 - 32, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, rightPixel);
        return leftPixel[1];
      }, { timeout: 10_000 }).toBeGreaterThan(160);
      // Untextured rectangles expose normalized local UVs to procedural shaders.
      expect(rightPixel[1]).toBeGreaterThan(leftPixel[1] + 25);
      gl.readPixels(136, 100 - 1 - 32, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, secondPixel);
      expect(secondPixel[0]).toBeGreaterThan(200);
      expect(secondPixel[1]).toBeLessThan(10);
      expect(secondPixel[2]).toBeGreaterThan(200);
      // One read for the missing declaration, one for the mistyped declaration,
      // three for the first valid block, none for the cached repeat, and three
      // fresh reads after hot replacement advances the program revision.
      expect(uniformTypeReads).toBe(8);
    } finally {
      cleanup();
    }
  });

  it('polls each external shader source revision once and recovers on a later edit', async () => {
    const shaderSources = createWaluauShaderSourceHost({ pixel: 'initial source' });
    let pixelRevisionReads = 0;
    await compileAndInstantiate(
      {
        '/fixtures/game-engine/shader-sources.walu': gameEngineShaderSources,
        '/engine/shader_sources.walu': gameEngineShaderSourcesModule,
      },
      '/fixtures/game-engine/shader-sources.walu',
      {
        shaderSources,
        hostImports: {
          __waluau_shader_source_revision(name) {
            if (name === 'pixel') {
              pixelRevisionReads += 1;
              if (pixelRevisionReads === 3) {
                shaderSources.update('pixel', 'bad shader source');
              } else if (pixelRevisionReads === 5) {
                shaderSources.update('pixel', 'recovered shader source');
              }
            }
            return shaderSources.imports.__waluau_shader_source_revision(name);
          },
          __waluau_shader_source_text: shaderSources.imports.__waluau_shader_source_text,
        },
      },
    );
    expect(pixelRevisionReads).toBe(5);
  });

  it('uploads images, batches sprites, and composites a transparent offscreen target', async () => {
    const steps = [];
    const asyncErrors = [];
    const atlasPixels = new Uint8ClampedArray([
      255, 0, 0, 255, 0, 255, 0, 255,
      0, 0, 255, 255, 255, 255, 255, 255,
    ]);
    let fetchCount = 0;
    let decodeCount = 0;
    const { root, cleanup } = await compileAndInstantiateWithDom(
      {
        '/fixtures/game-engine/gpu-resources.walu': gameEngineGpuResources,
        '/engine/browser.walu': gameEngineBrowser,
        '/engine/graphics.walu': gameEngineGraphics,
        '/engine/font.walu': gameEngineFont,
        '/engine/input.walu': gameEngineInput,
        '/engine/time.walu': gameEngineTime,
        '/engine/resources.walu': gameEngineResources,
      },
      '/fixtures/game-engine/gpu-resources.walu',
      {
        gameServices: {
          assetBaseUrl: 'https://game.test/',
          fetch: async () => {
            fetchCount += 1;
            return new Response(new Uint8Array([1]), { status: 200 });
          },
          createImageBitmap: async () => {
            decodeCount += 1;
            return createImageBitmap(new ImageData(atlasPixels, 2, 2));
          },
        },
        hostImports: {
          record_gpu_resource_step: (step) => steps.push(String(step)),
        },
        onAsyncError: (error) => asyncErrors.push(error),
      },
    );
    try {
      const canvas = root.querySelector('#walua-game-canvas');
      const gl = canvas.getContext('webgl2');
      const pixelAt = (x, y) => {
        const pixel = new Uint8Array(4);
        gl.readPixels(x, 64 - 1 - y, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
        return Array.from(pixel);
      };
      await expect.poll(() => steps, { timeout: 10_000 }).toContain('rendered');
      expect(asyncErrors).toEqual([]);
      expect(steps).toEqual(['decoded', 'uploaded', 'rendered']);
      expect(fetchCount).toBe(1);
      expect(decodeCount).toBe(1);
      expect(pixelAt(20, 20)).toEqual([255, 0, 0, 255]);
      expect(pixelAt(56, 20)).toEqual([0, 255, 0, 255]);
      expect(pixelAt(90, 14)).toEqual([0, 0, 0, 255]);
      expect(pixelAt(100, 20)).toEqual([0, 0, 255, 255]);
    } finally {
      cleanup();
    }
  });

  it('renders packaged image and custom-font resources with safe failure and release', async () => {
    const steps = [];
    const asyncErrors = [];
    const atlasPixels = new Uint8ClampedArray([
      255, 32, 32, 255, 32, 255, 32, 255,
      32, 32, 255, 255, 255, 255, 255, 255,
    ]);
    const { root, cleanup } = await compileAndInstantiateWithDom(
      {
        '/fixtures/game-engine/gpu-font-resources.walu': gameEngineGpuFontResources,
        '/engine/browser.walu': gameEngineBrowser,
        '/engine/graphics.walu': gameEngineGraphics,
        '/engine/font.walu': gameEngineFont,
        '/engine/input.walu': gameEngineInput,
        '/engine/time.walu': gameEngineTime,
        '/engine/resources.walu': gameEngineResources,
      },
      '/fixtures/game-engine/gpu-font-resources.walu',
      {
        gameServices: {
          assetBaseUrl: 'https://game.test/dist/',
          assetManifest: {
            'assets/card-back.svg': { url: './assets/card-back.hash.svg', type: 'image' },
            'assets/Cinzel-Bold.ttf': { url: './assets/vault.hash.ttf', type: 'font' },
            'assets/missing.svg': { url: './assets/missing.hash.svg', type: 'image' },
          },
          fetch: async (url) => {
            if (url.endsWith('card-back.hash.svg')) {
              return new Response(pokerCardBack, {
                status: 200,
                headers: { 'Content-Type': 'image/svg+xml' },
              });
            }
            if (url.endsWith('vault.hash.ttf')) return fetch(pokerFontUrl);
            return new Response('', { status: 404 });
          },
          createImageBitmap: async () => createImageBitmap(new ImageData(atlasPixels, 2, 2)),
        },
        hostImports: {
          record_gpu_font_step: (step) => steps.push(String(step)),
        },
        onAsyncError: (error) => asyncErrors.push(error),
      },
    );
    try {
      const canvas = root.querySelector('#walua-game-canvas');
      const gl = canvas.getContext('webgl2');
      await expect.poll(() => steps, { timeout: 10_000 }).toContain('released');
      expect(asyncErrors).toEqual([]);
      expect(steps).toEqual(['ready-and-failure', 'rendered', 'released']);

      const imagePixel = new Uint8Array(4);
      gl.readPixels(16, 64 - 1 - 16, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, imagePixel);
      expect(Array.from(imagePixel)).toEqual([255, 32, 32, 255]);

      const pixels = new Uint8Array(220 * 64 * 4);
      gl.readPixels(0, 0, 220, 64, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
      let cyanPixels = 0;
      for (let index = 0; index < pixels.length; index += 4) {
        if (pixels[index] >= 32 && pixels[index] <= 96 && pixels[index + 1] > 180 && pixels[index + 2] > 180) {
          cyanPixels += 1;
        }
      }
      expect(cyanPixels).toBeGreaterThan(20);
    } finally {
      cleanup();
    }
  });

  it('passes DOM mutation and localStorage host API checks', async () => {
    const testCase = cases.find(({ name }) => name === 'dom_storage_host_api.walu');
    const source = sourceForCase(testCase);
    const { root, storage } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');

    expect(root.children).toHaveLength(0);
    expect(storage.getItem('waluau-dom-storage-key')).toBeNull();
  });

  it('passes DOM Selection snake_case member rename checks', async () => {
    const testCase = cases.find(({ name }) => name === 'dom_selection_member_rename.walu');
    const source = sourceForCase(testCase);
    const { root, cleanup } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');

    try {
      expect(root.tagName).toBe('BODY');
    } finally {
      cleanup();
    }
  });

  it('runs the 2D engine clock and input simulation without a DOM', async () => {
    await compileAndInstantiate(
      {
        '/fixtures/game-engine/sim.walu': gameEngineSim,
        '/engine/input.walu': gameEngineInput,
        '/engine/time.walu': gameEngineTime,
      },
      '/fixtures/game-engine/sim.walu'
    );
  });

  it('runs the particle system simulation without a canvas', async () => {
    await compileAndInstantiate(
      {
        '/fixtures/particles/sim.walu': particleSim,
        '/engine/particles.walu': gameEngineParticles,
        '/engine/graphics.walu': gameEngineGraphics,
        '/engine/resources.walu': gameEngineResources,
        '/engine/font.walu': gameEngineFont,
      },
      '/fixtures/particles/sim.walu'
    );
  });

  it('renders the particle gallery, including its render-target sprite atlas', async () => {
    const { root, cleanup } = await compileAndInstantiateWithDom(
      {
        '/fixtures/particles/main.walu': particleGallery,
        '/fixtures/particles/scenes.walu': particleScenes,
        '/engine/browser.walu': gameEngineBrowser,
        '/engine/graphics.walu': gameEngineGraphics,
        '/engine/particles.walu': gameEngineParticles,
        '/engine/resources.walu': gameEngineResources,
        '/engine/font.walu': gameEngineFont,
        '/engine/input.walu': gameEngineInput,
        '/engine/time.walu': gameEngineTime,
      },
      '/fixtures/particles/main.walu'
    );

    try {
      const canvas = root.querySelector('#walua-game-canvas');
      expect(canvas).not.toBeNull();
      expect(canvas.width).toBe(960);
      expect(canvas.height).toBe(540);
      const gl = canvas.getContext('webgl2');
      expect(gl).not.toBeNull();

      const litPixels = () => {
        const pixels = new Uint8Array(canvas.width * canvas.height * 4);
        gl.readPixels(0, 0, canvas.width, canvas.height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
        // The campfire is additive and warm, so count pixels well above the
        // scene's near-black background.
        let lit = 0;
        for (let i = 0; i < pixels.length; i += 4) {
          if (pixels[i] > 120 && pixels[i] > pixels[i + 2] + 40) lit += 1;
        }
        return lit;
      };

      // The first scene emits continuously; give the loop a few frames.
      await expect.poll(litPixels, { timeout: 10_000 }).toBeGreaterThan(200);

      // Scene 8 draws particles from an atlas built into a render target,
      // which only produces pixels if the whole texture path works.
      root.ownerDocument.dispatchEvent(new root.ownerDocument.defaultView.KeyboardEvent('keydown', { key: '8' }));

      const spritePixels = () => {
        const pixels = new Uint8Array(canvas.width * canvas.height * 4);
        gl.readPixels(0, 0, canvas.width, canvas.height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
        let lit = 0;
        for (let i = 0; i < pixels.length; i += 4) {
          if (pixels[i + 2] > 140 && pixels[i + 1] > 100) lit += 1;
        }
        return lit;
      };
      await expect.poll(spritePixels, { timeout: 10_000 }).toBeGreaterThan(100);

      // Scene 9 is the ash fall, which is built entirely from the sway force
      // and per-particle color variation. Its embers rise additively out of
      // the bottom of an almost black frame.
      root.ownerDocument.dispatchEvent(new root.ownerDocument.defaultView.KeyboardEvent('keydown', { key: '9' }));
      await expect.poll(litPixels, { timeout: 10_000 }).toBeGreaterThan(100);
    } finally {
      cleanup();
    }
  });

  it('runs the resource, audio and save-data contract sample', async () => {
    let completed = 0;
    const asyncErrors = [];
    await compileAndInstantiate(
      {
        '/fixtures/game-engine/resources.walu': gameEngineResourceSample,
        '/engine/resources.walu': gameEngineResources,
        '/engine/audio.walu': gameEngineAudio,
        '/engine/save.walu': gameEngineSave,
      },
      '/fixtures/game-engine/resources.walu',
      {
        hostImports: {
          game_resource_load_text: (path) => Promise.resolve(path.includes('missing') ? 7 : 1),
          game_resource_load_bytes: () => Promise.resolve(2),
          game_resource_load_image: () => Promise.resolve(3),
          game_resource_load_font: () => Promise.resolve(4),
          game_audio_load_sound: () => Promise.resolve(5),
          game_audio_load_stream: () => Promise.resolve(6),
          game_save_write_text: () => Promise.resolve(8),
          game_save_read_text: () => Promise.resolve(9),
          game_resource_ok: (handle) => handle !== 7,
          game_resource_error_code: (handle) => handle === 7 ? 'not_found' : '',
          game_resource_error_message: () => 'missing packaged asset',
          game_resource_text: (handle) => handle === 1 ? 'packaged text' : 'breach=3',
          game_resource_bytes: () => new Uint8Array([1, 2, 3, 4]),
          game_resource_font_family: () => 'Sample Game',
          game_resource_release: () => {},
          game_audio_play: () => true,
          game_audio_pause: () => {},
          game_audio_stop: () => {},
          game_audio_is_playing: () => false,
          record_resource_sample_complete: () => { completed += 1; },
        },
        onAsyncError: (error) => { asyncErrors.push(error); },
      },
    );

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(asyncErrors).toEqual([]);
    expect(completed).toBe(1);
  });
});
