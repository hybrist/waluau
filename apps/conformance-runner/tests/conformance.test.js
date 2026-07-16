import { describe, it, expect } from 'vitest';
import {
  compileAndInstantiate,
  compileAndInstantiateWithDom,
  compileAndInstantiateWithExports,
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
import gameEngineBrowser from '../../../engine/browser.walu?raw';
import gameEngineGraphics from '../../../engine/graphics.walu?raw';
import gameEngineFont from '../../../engine/font.walu?raw';
import gameEngineInput from '../../../engine/input.walu?raw';
import gameEngineTime from '../../../engine/time.walu?raw';
import arcaneHeistSim from '../../../fixtures/poker-tricks/sim.walu?raw';
import arcaneHeistGame from '../../../fixtures/poker-tricks/game.walu?raw';

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
  for (const { name, source } of cases) {
    if (DEDICATED_ASYNC_DOM_CASES.has(name)) continue;

    const { pending, expectedErrors } = conformanceExpectations(source);
    const fullSource = sourceForCase({ name, source });
    const options = optionsForCase(name);

    if (expectedErrors.length > 0 && pending) {
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
    } else if (pending) {
      // Pending test: should pass eventually but does not yet. Only verify it
      // currently fails; we do not care how.
      it(`pending ${name} (currently fails)`, async () => {
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

  it('passes type aliases imported across require boundaries', async () => {
    const state = `
      type State = { score: i32 }

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
    exports.run_rejected();
    await harness.flush();

    expect(harness.strings).toEqual(['settled']);
    expect(harness.objects).toEqual([harness.objectValue]);
    expect(harness.nested).toEqual(['settled:inner-yield:17']);
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

  it('measures and aligns game-engine text in logical pixels', async () => {
    const { root, cleanup } = await compileAndInstantiateWithDom(
      {
        '/fixtures/game-engine/text-alignment.walu': gameEngineTextAlignment,
        '/engine/browser.walu': gameEngineBrowser,
        '/engine/graphics.walu': gameEngineGraphics,
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

  // The fixture's rules module is host-independent, so its seeded assertions
  // run here with no engine and no DOM; any of them trapping fails this test.
  it('runs the Arcane Heist rules simulation without a DOM', async () => {
    await compileAndInstantiate(
      {
        '/fixtures/poker-tricks/sim.walu': arcaneHeistSim,
        '/fixtures/poker-tricks/game.walu': arcaneHeistGame,
      },
      '/fixtures/poker-tricks/sim.walu'
    );
  });
});
