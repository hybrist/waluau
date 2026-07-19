// Meta-tests for the walu-test vitest bridge (tools/walu-test/host.js).
// A recording fake stands in for vitest's registration API so we can verify
// suite structure, hook wiring, matcher behavior, and failure propagation —
// including paths that must fail, which the demo *.test.walu suites cannot
// express. Matchers still run through the real vitest `expect`.
import { describe, it, expect } from 'vitest';
import { registerWaluTests } from '../../../tools/walu-test/host.js';

async function loadCompiler() {
  const module = await import('./../src/waluau-wasm/waluau_wasm.js');
  return { init: module.default, compile_multi: module.compile_multi };
}

function createRecordingApi() {
  const events = [];
  const tests = [];
  const hooks = { beforeEach: [], afterEach: [], beforeAll: [], afterAll: [] };

  const describeFake = (name, body) => {
    events.push(`describe:${name}`);
    body();
    events.push(`describe-end:${name}`);
  };
  describeFake.skip = (name) => {
    events.push(`describe.skip:${name}`);
  };
  const itFake = (name, body) => {
    events.push(`it:${name}`);
    tests.push({ name, body });
  };
  itFake.skip = (name) => {
    events.push(`it.skip:${name}`);
  };
  itFake.todo = (name) => {
    events.push(`it.todo:${name}`);
  };

  return {
    events,
    tests,
    hooks,
    api: {
      describe: describeFake,
      it: itFake,
      beforeEach: (body) => hooks.beforeEach.push(body),
      afterEach: (body) => hooks.afterEach.push(body),
      beforeAll: (body) => hooks.beforeAll.push(body),
      afterAll: (body) => hooks.afterAll.push(body),
      expect,
    },
  };
}

async function register(source, recorder) {
  const { init, compile_multi } = await loadCompiler();
  return registerWaluTests({
    source,
    path: '/host-meta.test.walu',
    init,
    compile_multi,
    api: recorder.api,
  });
}

describe('walu-test host bridge', () => {
  it('registers nested suites, tests, hooks, and skip/todo entries', async () => {
    const recorder = createRecordingApi();
    await register(
      `
describe("outer", function(): unit
    before_each(function(): unit
    end)
    it("first", function(): unit
        expect(1):toBe(1)
    end)
    describe("inner", function(): unit
        it("second", function(): unit
            expect(2):toBe(2)
        end)
    end)
end)
xdescribe("skipped suite", function(): unit
    it("hidden", function(): unit
    end)
end)
xit("skipped test", function(): unit
end)
todo("later")
test("aliased", function(): unit
end)
`,
      recorder,
    );

    expect(recorder.events).toEqual([
      'describe:outer',
      'it:first',
      'describe:inner',
      'it:second',
      'describe-end:inner',
      'describe-end:outer',
      'describe.skip:skipped suite',
      'it.skip:skipped test',
      'it.todo:later',
      'it:aliased',
    ]);
    expect(recorder.hooks.beforeEach).toHaveLength(1);
  });

  it('runs test bodies through the unit-callback trampoline', async () => {
    const recorder = createRecordingApi();
    await register(
      `
describe("state", function(): unit
    local counter: i32 = 0
    before_each(function(): unit
        counter = counter + 1
    end)
    it("sees hook effects", function(): unit
        expect(counter):toBeGreaterThanOrEqual(1)
    end)
    it("sees repeated hook effects", function(): unit
        expect(counter):toBe(2)
    end)
end)
`,
      recorder,
    );

    for (const test of recorder.tests) {
      for (const hook of recorder.hooks.beforeEach) hook();
      test.body();
    }
  });

  it('covers number, string, bool, and negated matchers', async () => {
    const recorder = createRecordingApi();
    await register(
      `
it("matchers", function(): unit
    expect(4):toBe(4)
    expect(4):notToBe(5)
    expect(0.30000001):toBeCloseTo(0.3)
    expect(0.31):toBeCloseTo(0.3, 1)
    expect(7):toBeGreaterThan(6)
    expect(7):toBeGreaterThanOrEqual(7)
    expect(7):toBeLessThan(8)
    expect(7):toBeLessThanOrEqual(7)
    expect("waluau"):toBe("waluau")
    expect("waluau"):notToBe("lua")
    expect("waluau"):toContain("alu")
    expect("waluau"):notToContain("moon")
    expect("waluau"):toHaveLength(6)
    expect(true):toBe(true)
    expect(1 == 1):toBeTruthy()
    expect(1 == 2):toBeFalsy()
end)
`,
      recorder,
    );

    expect(recorder.tests).toHaveLength(1);
    recorder.tests[0].body();
  });

  it('propagates vitest assertion failures out of test bodies', async () => {
    const recorder = createRecordingApi();
    await register(
      `
it("fails", function(): unit
    expect(1):toBe(2)
end)
it("fails on booleans", function(): unit
    expect(true):toBe(false)
end)
`,
      recorder,
    );

    expect(() => recorder.tests[0].body()).toThrowError(/expected 1 to be 2/);
    // Booleans cross the wasm boundary as 0/1; the bridge maps them back so
    // the failure message talks about true/false, not numbers.
    expect(() => recorder.tests[1].body()).toThrowError(/expected true to be false/);
  });

  it('surfaces Lua assert messages as readable JS errors', async () => {
    const recorder = createRecordingApi();
    await register(
      `
it("lua assert", function(): unit
    assert(1 == 2, "one is not two")
end)
`,
      recorder,
    );

    expect(() => recorder.tests[0].body()).toThrowError(/one is not two/);
  });

  it('reports compile errors at registration time', async () => {
    const recorder = createRecordingApi();
    await expect(
      register(
        `
it("broken", function(): unit
    expect(1):toBe("not a number")
end)
`,
        recorder,
      ),
    ).rejects.toBeTruthy();
  });
});
