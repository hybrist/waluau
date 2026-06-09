import { describe, it, expect } from 'vitest';
import {
  compileAndInstantiate,
  compileAndInstantiateWithDom,
  compileAndInstantiateWithExports,
} from '../src/runner.js';
import { conformanceIncludePaths } from '../../../tools/conformance/includes.js';

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

const cases = Object.entries(conformanceModules)
  .map(([path, source]) => {
    const normalized = path.replace(/^.*\/conformance\//, '');
    return { name: normalized, source };
  })
  .sort((a, b) => a.name.localeCompare(b.name));

// Cases that have a dedicated test below (async DOM start functions that need
// the iframe to stay alive until the async work completes).
const DEDICATED_ASYNC_DOM_CASES = new Set(['top_level_fetch.walu']);

describe('browser conformance', () => {
  for (const { name, source } of cases) {
    if (DEDICATED_ASYNC_DOM_CASES.has(name)) continue;
    it(`passes ${name}`, async () => {
      await expect(
        compileAndInstantiate(
          { '/main.walu': sourceForCase({ name, source }) },
          '/main.walu',
          optionsForCase(name),
        ),
      ).resolves.toBeUndefined();
    });
  }

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

  it('passes top_level_fetch.walu with async start-function fetch and DOM write', async () => {
    const testCase = cases.find(({ name }) => name === 'top_level_fetch.walu');
    const source = sourceForCase(testCase);
    const { root, cleanup } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');

    try {
      // The Wasm start function launches the coroutine and returns; the fetch
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
});
