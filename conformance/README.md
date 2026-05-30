# Waluau conformance tests

Each `*.walu` file in this directory (and its subdirectories) is a
self-contained conformance test for the Waluau language.

## How a test works

A conformance test is an ordinary Waluau program that exercises some behaviour
and checks the results with top-level `assert(...)` statements. Top-level
statements run during module instantiation (via the WebAssembly `start`
section), so the harness only has to compile each program and instantiate it:

- The test **passes** if the program compiles and instantiates without
  trapping.
- The test **fails** if the program fails to compile, or if any top-level
  `assert(...)` evaluates to `false` (which traps during instantiation).

Because the checks are written in Waluau itself, a test reads like the feature
it documents:

```lua
function add(a: i32, b: i32): i32
    return a + b
end

assert(add(2, 3) == 5)
assert(add(-1, 1) == 0)
```

## Adding a test

1. Create a new `*.walu` file in this directory (group related files into a
   subdirectory if it helps).
2. Use top-level `assert(...)` calls to check the behaviour you care about.
   Keep each file focused on one feature so a failure points at the cause.
3. Run the suite (see below). New files are picked up automatically — no
   registration step.

## Running the suite

```bash
cargo test -p waluau-driver --test conformance
```

The harness lives in `crates/waluau-driver/tests/conformance.rs`. It discovers
every `*.walu` file under this directory, compiles and instantiates each, and
reports which ones passed or failed.
