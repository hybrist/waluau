# @waluau/vite-plugin

This plugin makes `.walu` files ordinary Vite modules. Importing one compiles
the Waluau project rooted at that file, supplies the standard browser imports,
starts the game, and exports its loading promise as both `game` and the default
export. The generated game owns the entire viewport by default.

```js
// vite.config.js
import { defineConfig } from 'vite';
import { waluau } from '@waluau/vite-plugin';

export default defineConfig({
  plugins: [waluau()],
});
```

Point a normal module script at the Waluau source:

```html
<script type="module" src="/src/main.walu"></script>
```

The same file can be imported from JavaScript with
`import game from './main.walu'`. Set `fullScreen: false` to embed the game
without the viewport styles. Outside the Waluau repository, install the
`waluau` compiler binary or pass a custom `compiler` command.

## Testing with vitest

Files named `*.test.walu` become vitest test modules instead of games: the
plugin compiles them the same way, then registers their suites with vitest
during collection. Pull in the test API with `require("waluau:vitest")` and
write busted-style suites with vitest-style matchers:

```lua
require("waluau:vitest")

function add(a: i32, b: i32): i32
    return a + b
end

describe("arithmetic", function(): unit
    before_each(function(): unit
    end)

    it("adds integers", function(): unit
        expect(add(2, 2)):toBe(4)
        expect(0.1 + 0.2):toBeCloseTo(0.3)
        expect("hello walu"):toContain("walu")
        expect(add(1, 1) == 2):toBeTruthy()
    end)
end)
```

The API (declared in `externs/vitest.walu`): `describe`, `it`, `test`,
`xdescribe`/`xit` (skip), `todo(name)`, the hooks `before_each`,
`after_each`, `before_all`, `after_all`, and `expect(value)` whose matcher
set follows the value's static type (numbers: `toBe`, `notToBe`,
`toBeCloseTo`, `toBeGreaterThan[OrEqual]`, `toBeLessThan[OrEqual]`; strings:
`toBe`, `notToBe`, `toContain`, `notToContain`, `toHaveLength`; booleans:
`toBe`, `toBeTruthy`, `toBeFalsy`; externs: identity `toBe`/`notToBe`).
Failed Waluau `assert(cond, msg)` calls surface as readable vitest failures
with correct file/line info.

Wire vitest (browser mode) into the same vite config and include the test
files:

```js
export default defineConfig({
  plugins: [waluau()],
  test: {
    include: ['src/**/*.test.walu'],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: [{ browser: 'chromium' }],
    },
  },
});
```

`apps/ante` is wired up this way. The bridge itself lives in
`@waluau/vite-plugin/testing`; its meta-tests (and a browser-compiled
variant used by the conformance runner) live in `apps/conformance-runner`.
