# walu-test

Busted-style testing for Waluau, running on vitest. Test files are plain
`.walu` sources named `*.test.walu`; they compile in the browser and register
their suites with vitest during collection, so vitest owns running, reporting,
watch mode, and filtering.

```lua
function add(a: i32, b: i32): i32
    return a + b
end

describe("arithmetic", function(): unit
    local counter: i32 = 0

    before_each(function(): unit
        counter = counter + 1
    end)

    it("adds integers", function(): unit
        expect(add(2, 2)):toBe(4)
    end)

    it("compares and matches", function(): unit
        expect(0.1 + 0.2):toBeCloseTo(0.3)
        expect("hello walu"):toContain("walu")
        expect(add(1, 1) == 2):toBeTruthy()
    end)
end)
```

## API

Declared in [`externs/vitest.walu`](../../externs/vitest.walu), which the vite
plugin appends to every test file — no `require` needed:

- **Suites:** `describe`, `it`, `test` (alias), `xdescribe`/`xit` (skip),
  `todo(name)`
- **Hooks (busted naming):** `before_each`, `after_each`, `before_all`,
  `after_all`
- **Expectations:** `expect(value)` with vitest-style matchers. The
  expectation type follows the value's static type:
  - numbers: `toBe`, `notToBe`, `toBeCloseTo(expected[, digits])`,
    `toBeGreaterThan[OrEqual]`, `toBeLessThan[OrEqual]`
  - strings: `toBe`, `notToBe`, `toContain`, `notToContain`, `toHaveLength`
  - booleans: `toBe`, `toBeTruthy`, `toBeFalsy`
  - externs: `toBe`, `notToBe` (JS identity)

Failed Waluau `assert(cond, msg)` calls inside a test body also produce
readable vitest failures with correct file/line info, via the module's
exported `__waluau_error_tag`.

## Wiring into an app

The app needs vitest (browser mode) and a built waluau-wasm compiler module
(see `apps/playground/vite-plugin-waluau-wasm.js`). Then:

```js
// vite.config.js
import { waluTestPlugin } from '../../tools/walu-test/vite-plugin.js';

export default defineConfig({
  plugins: [
    waluauWasmPlugin({ ... }),
    waluTestPlugin({
      waluauWasmPath: resolve(appRoot, 'src/waluau-wasm/waluau_wasm.js'),
    }),
  ],
  test: {
    include: ['tests/**/*.test.js', 'tests/**/*.test.walu'],
    // browser mode config...
  },
});
```

`apps/conformance-runner` is wired up this way; see
`tests/walu-demo.test.walu` for a working suite and
`tests/walu-test-host.test.js` for the bridge's meta-tests.

## How it works

- The vite plugin (`vite-plugin.js`) loads each `*.test.walu` file as a JS
  module that calls `registerWaluTests` from `host.js`.
- `host.js` appends the `externs/vitest.walu` declarations (appending keeps
  the test file's own line numbers accurate), compiles with `compile_multi`,
  and instantiates with host imports that map `describe`/`it`/hooks/matchers
  onto the real vitest API.
- Suite/test/hook bodies are Waluau `() -> unit` closures; the host invokes
  them through the module's exported `__waluau_call_callback_unit`
  trampoline. Test bodies run lazily when vitest runs the test, against the
  same live wasm instance, so state shared through captured locals (e.g.
  `before_each` counters) behaves as expected.
- Booleans cross the wasm boundary as 0/1 integers; the per-type expectation
  externs let the bridge map them back to `true`/`false` before calling
  vitest's `expect`, so failure messages stay truthful.
