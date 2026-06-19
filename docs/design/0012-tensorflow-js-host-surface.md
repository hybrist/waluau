# 0012: TensorFlow.js Host Surface and Tensor Semantics

## Status

Draft.

## Goal

Define the first TensorFlow.js-backed tensor programming surface for Waluau
before runtime/compiler implementation. This design is intentionally limited to
the host surface, type semantics, first-slice operator lowering, lifetime rules,
dependency order, and test plan. It does not implement TensorFlow.js support.

The design is grounded in the existing Waluau host model:

- host objects are nominal `type T = extern` values backed by Wasm `externref`
  (see [0011](0011-dom-api-support.md));
- host functions and extern methods are source-level `declare function` imports;
- virtual modules such as `require("dom:window")` are resolved by the linker and
  merged into the program before HIR/IR/codegen see it;
- generic extern declarations such as `type Promise<T> = extern` already erase
  to externref-compatible host imports.

It is also grounded in TensorFlow.js behavior:

- `tf.Tensor` has `rank`, `shape`, and `dtype` metadata;
- tensors are immutable, and arithmetic/math operations return new tensors;
- tensor data readback has async APIs (`data()`, `array()`) and sync APIs
  (`dataSync()`, `arraySync()`), with async preferred for production;
- TensorFlow.js has one active backend at a time and supports backends such as
  `webgl`, `wasm`, and `cpu`;
- host memory must be managed explicitly with `dispose()` or `tf.tidy()`.

References:

- <https://www.tensorflow.org/js/guide/tensors_operations>
- <https://www.tensorflow.org/js/guide/platform_environment>
- <https://js.tensorflow.org/api/latest/>

## Surface Module Shape

The TensorFlow.js surface is a virtual module named `tfjs`:

```lua
local tf = require("tfjs")
```

The linker should treat `require("tfjs")` like `require("dom:window")`: it is a
virtual specifier, not a filesystem path. Requiring it injects a built-in
ambient extern declaration file, e.g. `externs/tfjs.walu`, and rewrites the
require expression to a namespace value with fixed fields. A program that never
requires `tfjs` should not receive TensorFlow.js imports or diagnostics.

The namespace should be a table of functions in the same style as existing
module table exports. First slice:

```lua
local tf = require("tfjs")

local a: Tensor = tf.tensor2d({ 1.0, 2.0, 3.0, 4.0 }, 2, 2)
local b: Tensor = tf.eye(2)
local c: Tensor = tf.matmul(a, b)
```

The runtime host import module should remain `"waluau"` unless a broader host
module namespacing change lands first. TensorFlow.js imports should use stable
host names prefixed with `tfjs.`:

```lua
declare function tfjs_tensor1d(data: {f64}, dtype: string): Tensor -- host "tfjs.tensor1d"
```

If implementation supports explicit import modules before this work starts, the
same declarations may instead import from module `"tfjs"` with names such as
`"tensor1d"`. Downstream code must pick one convention once, expose it through
declared imports, and keep the generated Wasm imports stable.

## Extern Tensor Type Semantics

First slice declarations:

```lua
type Tensor = extern
type TensorData = extern
```

`Tensor` is nominal and opaque in Waluau. It represents a live JavaScript
`tf.Tensor` object. It is assignable only to `Tensor`, nullable `Tensor?` where
that is explicitly used, or wider extern supertypes if a future design adds one.
It is not assignable to numbers, arrays, records, `bytes`, or `unknown` without
the existing explicit extern/unknown mechanisms.

The type system must treat `Tensor` as immutable:

- no Waluau field writes on `Tensor`;
- arithmetic and math functions return new `Tensor` values;
- operations do not mutate their inputs;
- aliasing a `Tensor` only aliases the host handle, not mutable Waluau data.

`TensorData` is a host object used for readback of flat numeric data. It is
separate from `Tensor` so data readback can return a host typed array without
pretending Waluau arrays are views over JavaScript memory. The first slice only
needs `TensorData` for tests and simple inspection.

Do not model `tf.Variable` in the first slice. Variables are mutable and have
assignment/training semantics that would confuse the immutable `Tensor` story.

## DType and Shape Representation

Use simple Waluau values at the boundary:

- `dtype` is a string literal/value accepted by host constructors;
- supported dtype strings in the first slice are `"float32"`, `"int32"`, and
  `"bool"`;
- omitted dtype defaults to `"float32"` through convenience wrappers;
- `shape` is explicit integer dimensions on rank-specific constructors, not a
  general rank-N Waluau array.

Rationale:

- TFJS uses dtype strings and shape arrays internally, so this mirrors the host
  API without adding new Waluau enum or variadic features.
- Rank-specific constructors avoid depending on a fully ergonomic `{i32}` or
  `Shape` abstraction before tensor interop exists.
- Flat data plus explicit dimensions makes row-major order visible and testable.

First-slice constructors:

```lua
tf.scalar(value: f64): Tensor
tf.scalar_i32(value: i32): Tensor
tf.scalar_bool(value: bool): Tensor
tf.tensor1d(data: {f64}): Tensor
tf.tensor1d_i32(data: {i32}): Tensor
tf.tensor2d(data: {f64}, rows: i32, cols: i32): Tensor
tf.tensor2d_i32(data: {i32}, rows: i32, cols: i32): Tensor
tf.zeros(rows: i32, cols: i32): Tensor
tf.ones(rows: i32, cols: i32): Tensor
tf.eye(size: i32): Tensor
```

The runtime must validate shape/data length at the host boundary and report a
clear trap or JS exception through the existing runtime error path. For
`tensor2d`, data length must equal `rows * cols`. Dimensions must be
non-negative. Empty dimensions should follow TFJS behavior once tested, but are
not required for first-slice examples.

General `tf.tensor(data, shape, dtype)` and rank > 2 constructors are deferred
until Waluau has a better shape representation or typed array interop.

## Readback APIs

Readback must include both a testing-friendly synchronous path and a
production-compatible async path.

First slice:

```lua
tf.data(t: Tensor): Promise<TensorData>
tf.data_sync(t: Tensor): TensorData
tf.data_len(data: TensorData): i32
tf.data_get_f64(data: TensorData, index: i32): f64
tf.data_get_i32(data: TensorData, index: i32): i32
tf.scalar_value(t: Tensor): f64
tf.scalar_value_i32(t: Tensor): i32
tf.shape_rank(t: Tensor): i32
tf.shape_dim(t: Tensor, index: i32): i32
tf.dtype(t: Tensor): string
```

`tf.data` maps to `Tensor.data()` and returns `Promise<TensorData>`, relying on
the existing `Promise<T>` extern pattern and `promise.await` support when a
program wants async readback. `tf.data_sync` maps to `Tensor.dataSync()` and is
permitted for conformance tests and small playground examples. The design should
document that sync readback may block the UI and should not be the default for
large production flows.

`tf.scalar_value` and `tf.scalar_value_i32` are convenience assertions over
rank-0 tensors. Host code must reject non-scalar tensors with a clear error.

Do not return nested Waluau arrays from `array()` / `arraySync()` in the first
slice. Nested shape-dependent values require either dynamic arrays-of-arrays,
records, or `unknown` policies that are unrelated to the MVP.

## Lifetime Semantics: dispose and tidy

TensorFlow.js tensors own backend resources. Waluau must not rely on JavaScript
garbage collection to release GPU/WASM memory.

First slice:

```lua
tf.dispose(t: Tensor): unit
tf.keep(t: Tensor): Tensor
tf.tidy(fn: () -> Tensor): Tensor
tf.memory_num_tensors(): i32
```

Semantics:

- `tf.dispose(t)` calls `t.dispose()`. Using a disposed `Tensor` in later host
  operations is a runtime error.
- `tf.tidy(fn)` calls `tf.tidy(() => ...)` around a Waluau callback. It returns
  the callback's returned tensor and disposes intermediate tensors created
  inside the callback unless they are returned or explicitly kept.
- `tf.keep(t)` maps to `tf.keep(t)` and is only needed when a tensor created in
  a tidy scope must escape without being the direct return value.
- `tf.memory_num_tensors()` exposes `tf.memory().numTensors` for leak-oriented
  conformance tests. It is a diagnostic/testing API, not a user-facing memory
  model guarantee.

`tf.tidy` must be synchronous in the first slice. TensorFlow.js tidy does not
support promises as the cleanup boundary; async tidy semantics are a separate
design problem. Programs that use async readback should do it outside a tidy
scope or keep returned tensors explicitly.

The compiler should not automatically insert `dispose` calls for overloaded
operators in this milestone. Automatic disposal needs ownership/liveness rules
that Waluau does not have yet. Downstream implementation should make leaks
visible through tests, then use explicit `tidy` in examples.

## First-Slice Operators and Function Mappings

Operator overloading must be statically resolved for `Tensor` operands and lower
to ordinary declared host calls. Existing scalar numeric operators keep their
current behavior.

Exact first-slice mappings:

| Waluau expression | Operand types | Result | TFJS operation |
|-------------------|---------------|--------|----------------|
| `a + b` | `Tensor`, `Tensor` | `Tensor` | `tf.add(a, b)` |
| `a - b` | `Tensor`, `Tensor` | `Tensor` | `tf.sub(a, b)` |
| `a * b` | `Tensor`, `Tensor` | `Tensor` | `tf.mul(a, b)` |
| `a / b` | `Tensor`, `Tensor` | `Tensor` | `tf.div(a, b)` |
| `-a` | `Tensor` | `Tensor` | `tf.neg(a)` |

No implicit number-to-tensor promotion is included in the first slice. `Tensor +
f64`, `f64 + Tensor`, and mixed dtype policies are rejected until an explicit
promotion design exists. Users can write `tf.scalar(2.0)` when they need a
scalar tensor.

Named functions required alongside operators:

```lua
tf.add(a: Tensor, b: Tensor): Tensor
tf.sub(a: Tensor, b: Tensor): Tensor
tf.mul(a: Tensor, b: Tensor): Tensor
tf.div(a: Tensor, b: Tensor): Tensor
tf.neg(a: Tensor): Tensor
tf.matmul(a: Tensor, b: Tensor): Tensor
tf.reshape2d(t: Tensor, rows: i32, cols: i32): Tensor
tf.transpose(t: Tensor): Tensor
```

The named arithmetic functions are not optional. They are needed for tests,
clear lowering, and users who do not want operator syntax. `tf.matmul` maps to
`tf.matMul(a, b)`. `*` is elementwise multiplication, not matrix multiplication.

Broadcasting should follow TFJS semantics for the mapped operations. Waluau does
not statically track tensor shapes in the first slice, so shape errors are host
runtime errors.

## Example MVP Program

```lua
local tf = require("tfjs")

function main(): f64
    local result: Tensor = tf.tidy(function(): Tensor
        local a: Tensor = tf.tensor2d({ 1.0, 2.0, 3.0, 4.0 }, 2, 2)
        local b: Tensor = tf.tensor2d({ 10.0, 20.0, 30.0, 40.0 }, 2, 2)
        local c: Tensor = tf.matmul(a + b, tf.eye(2))
        return c
    end)
    local values: TensorData = tf.data_sync(result)
    local first: f64 = tf.data_get_f64(values, 0)
    tf.dispose(result)
    return first
end
```

The free-function form is the required MVP. Method sugar may be added only if it
falls out naturally from the existing extern method machinery.

## Non-Goals

- Training, gradients, optimizers, layers, `tf.Variable`, or model loading.
- Automatic disposal/ownership inference for temporary tensors.
- Static shape checking, rank polymorphism, or dtype type parameters.
- General typed-array interop or zero-copy views into JavaScript memory.
- Nested array readback through `array()` / `arraySync()`.
- Node.js TensorFlow backend support beyond whatever the browser/playground host
  naturally exposes.
- Replacing Waluau numeric scalar semantics with tensor semantics.
- Matrix multiplication through `*`; `tf.matmul` is explicit.

## Dependency Order

| # | Work | Depends on | Notes |
|--:|------|------------|-------|
| 1 | Add/confirm `tfjs` virtual module resolution and ambient extern loading | existing require/linker support | Mirrors `dom:window`; no tensor runtime yet. |
| 2 | Add `externs/tfjs.walu` declarations | extern types, declared host functions, generic `Promise<T>` | Contains `Tensor`, `TensorData`, constructors, ops, readback, lifetime APIs. |
| 3 | Add browser/playground TFJS host binding | 1, 2 | Loads/provides TFJS, validates availability, maps host names to TFJS calls. |
| 4 | Add Tensor data interop | 2, 3 | Flat Waluau arrays to JS arrays/typed arrays; `TensorData` readback helpers. |
| 5 | Add static operator overloading for `Tensor` | 2, 3 | Lowers exact mappings in this note to host calls. |
| 6 | Add disposal/tidy conformance and examples | 3, 4, 5 | Uses `memory_num_tensors` to catch obvious leaks. |

Existing beads should be kept in that order: `waluau-vo3y` before deeper data
interop, `waluau-iyeh` before broad readback examples, `waluau-gaeg` before
operator syntax, and `waluau-tq4x` once tensor creation/ops can demonstrate
lifetime behavior.

## Test Plan

Compiler/linker tests:

- `require("tfjs")` resolves as a virtual module and injects `Tensor` /
  `TensorData` declarations.
- Unknown virtual specifiers near `tfjs` produce clear diagnostics.
- `Tensor` is nominally distinct from other extern types.
- Declared TFJS functions lower to host imports with stable names.
- Operator overloading accepts only the exact `Tensor` mappings listed above and
  rejects mixed `Tensor`/number operands.

Runtime/browser conformance:

- Create scalar, vector, and 2D tensors; assert dtype, rank, and dimensions.
- Read scalar and flat data through async `tf.data` plus `promise.await`.
- Read scalar and flat data through `tf.data_sync` for deterministic tests.
- Verify `+`, `-`, `*`, `/`, unary `-`, `tf.matmul`, `tf.reshape2d`, and
  `tf.transpose` against expected values.
- Verify TFJS broadcasting for one supported elementwise case and a clear
  runtime error for an invalid shape.
- Verify `tf.dispose` decrements `tf.memory().numTensors` for a simple tensor.
- Verify `tf.tidy` disposes intermediates while preserving the returned tensor.
- Verify `tf.keep` preserves a non-returned tensor from a tidy scope.
- Verify a helpful runtime diagnostic when `require("tfjs")` is used but the
  host did not provide TensorFlow.js.

Playground tests:

- Load the TFJS browser dependency before instantiating a Waluau program that
  imports `tfjs`.
- Run the same small example on the default backend.
- If backend selection is exposed in a later slice, test `webgl`, `wasm`, and
  `cpu` separately after `tf.ready()`. Backend switching is not required for
  this first design slice.
