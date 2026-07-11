# Waluau conformance tests

Each `*.walu` file in this directory (and its subdirectories) is a
self-contained conformance test for the Waluau language.

## How a test works

A conformance test is an ordinary Waluau program that exercises some behaviour
and checks the results with top-level `assert(...)` statements. Top-level
statements run through an exported `main` function, which the harness invokes
explicitly after instantiating the module:

- The test **passes** if the program compiles, instantiates, and its `main`
  entry point returns without trapping.
- The test **fails** if the program fails to compile, or if any top-level
  `assert(...)` evaluates to `false` (which traps during `main` execution).

Because the checks are written in Waluau itself, a test reads like the feature
it documents:

```lua
function add(a: i32, b: i32): i32
    return a + b
end

assert(add(2, 3) == 5)
assert(add(-1, 1) == 0)
```

## Test kinds

By default a file is a **pass** test as described above. Two directives, parsed
from the raw source (so they work even in files that intentionally don't
compile), opt a file into other kinds:

- `-- conformance: pending` marks a **pending** test: one that *should* pass
  eventually but doesn't yet. The runner only verifies the file currently fails
  (a compile/type error, or a trapping `assert`); it does not care how. When the
  feature lands and the file starts passing, the pending test goes red — that's
  the signal to remove the directive. See `string_sub.walu`.

- `-- conformance: error=<text>` marks a **fail** test: a file that must never
  pass (typically a syntax or type error, rarely a runtime trap). Each `error=`
  line is a required fragment of the failure message; the directive may be
  repeated to require several fragments. Matching is fuzzy — runs of whitespace
  are collapsed and each fragment must appear as a substring of the actual
  message. The runner verifies the file fails and that every fragment is
  present. See `unknown_type.walu`.

A file may carry both directives at once. A **fail + pending** test names the
failure it should eventually produce (`error=`) but doesn't produce it yet, so
the runner verifies the actual outcome does **not** match the expected failure
(the file may currently pass, or fail with a different message). When the
expected failure starts appearing, the test goes red — remove `pending`. See
`string_subtraction.walu`.

| `pending` | `error=` | Kind | Runner verifies |
|-----------|----------|------|-----------------|
| no  | no  | pass | compiles, instantiates, and executes without trapping |
| yes | no  | pending | currently fails (any way) |
| no  | yes | fail | fails, and every `error=` fragment is in the message |
| yes | yes | fail + pending | the expected failure is **not** produced (yet) |

## Adding a test

1. Create a new `*.walu` file in this directory (group related files into a
   subdirectory if it helps).
2. Use top-level `assert(...)` calls to check the behaviour you care about.
   Keep each file focused on one feature so a failure points at the cause.
3. Run the suite (see below). New files are picked up automatically — no
   registration step.

## Running the suite

```bash
pnpm --filter conformance-runner test:browser
```

The harness lives in `apps/conformance-runner`. It discovers every `*.walu`
file under this directory, compiles and instantiates each with
`WebAssembly.instantiate()` in a real browser (via Playwright), invokes its
exported `main` entry point, and reports
which ones passed or failed.
