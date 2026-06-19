# 0014: TensorFlow.js LayersModel Training Slice

## Status

Draft.

## Goal

Define the smallest useful TensorFlow.js `LayersModel` training surface after
URL-loaded model inference from [0013](0013-tensorflow-js-model-loading.md).
This note decides whether Waluau should expose a constrained `compile_sgd` plus
`fit_one` / `fit` API returning `Promise<TrainingHistory>`, and records the
callback, yielding, history, and lifetime rules that must be fixed before
runtime work starts.

The recommended next slice is intentionally narrow:

- compile an already-loaded `LayersModel` with SGD and a named built-in loss;
- train one input tensor against one target tensor with finite `epochs` and
  `batch_size`;
- return TFJS `History` as an opaque `TrainingHistory` extern wrapped in
  `Promise<TrainingHistory>`;
- expose read-only scalar loss history helpers;
- keep callbacks, validation, metrics, optimizer objects, datasets,
  `trainOnBatch`, `evaluate`, model/layer construction, and save/export as
  follow-up work.

References:

- <https://js.tensorflow.org/api/latest/#tf.LayersModel.compile>
- <https://js.tensorflow.org/api/latest/#tf.LayersModel.fit>
- <https://js.tensorflow.org/api/latest/#train.sgd>

## Current Baseline

PR #258 and PR #259 proved the host pieces needed for a small training call:

- `LayersModel` is a nominal extern type loaded from a URL.
- `Tensor` and `TensorData` already support small deterministic fixtures and
  scalar readback.
- `Promise<T>` plus `promise.await` works for model loading, async tensor
  readback, and `GraphModel.predictAsync`.
- Browser conformance and the playground already use checked-in TFJS model
  assets instead of remote URLs.
- Model and tensor disposal are explicit.

The missing pieces are not compiler primitives. They are TFJS host wrappers,
extern declarations, namespace wiring, conformance fixtures, and playground
coverage.

## Recommendation

Expose a constrained training slice, but name the first fit helper
`layers_model_fit_one` rather than `layers_model_fit`.

`fit_one` means "one input tensor and one target tensor", not "one epoch". That
keeps room for a later `layers_model_fit` that can accept validation data,
metrics, callbacks, or a structured options object if Waluau gains a better JS
options representation.

Recommended extern additions:

```lua
type TrainingHistory = extern

tf.layers_model_compile_sgd(
    model: LayersModel,
    loss: string,
    learning_rate: f64,
): unit

tf.layers_model_fit_one(
    model: LayersModel,
    x: Tensor,
    y: Tensor,
    epochs: i32,
    batch_size: i32,
): Promise<TrainingHistory>

tf.training_history_len(history: TrainingHistory): i32
tf.training_history_loss(history: TrainingHistory, epoch: i32): f64
```

Do not add `layers_model_fit` in the first implementation bead. Reserve that
name for the broader API. If the first slice needs a friendlier public alias,
add it only after the single-input/single-output semantics are proven in
browser conformance.

## Compile Surface

`layers_model_compile_sgd` maps to:

```js
model.compile({
  optimizer: tf.train.sgd(learningRate),
  loss,
});
```

Arguments:

- `model`: an undisposed `LayersModel`.
- `loss`: a string naming a TFJS built-in loss, such as
  `"meanSquaredError"`. The host wrapper passes this through to TFJS and lets
  TFJS reject unknown loss names. Waluau should not introduce a loss enum until
  there is enough API surface to justify generated bindings or dedicated loss
  helpers.
- `learning_rate`: a finite positive number. The host wrapper should reject
  `NaN`, infinity, zero, and negative values before creating the optimizer.

No metrics argument is included in the first slice. Metrics expand the history
shape and require generic key lookup or per-metric helpers. No optimizer object
extern is included; optimizer lifetime and configuration should remain owned by
the compiled model for now.

Calling `compile_sgd` more than once should follow TFJS behavior and replace
the compiled training configuration. Waluau should document that old optimizer
state is TFJS-owned and not separately disposable through this API.

## Optimizer and Loss Representation

Use a string loss and numeric learning rate first.

Rejected alternatives:

- `Optimizer` extern: too much lifetime and state surface for the first slice.
- `Loss` extern or enum: cleaner typing, but prematurely commits to a catalog
  and does not solve custom losses.
- JS function losses: would require callback ABI semantics during training and
  async error propagation.

The first slice should explicitly document that only TFJS built-in string
losses are supported. The conformance fixture should use
`"meanSquaredError"`.

## Fit Surface and Promise Behavior

`layers_model_fit_one` maps to:

```js
model.fit(x, y, {
  epochs,
  batchSize: batchSize,
  shuffle: false,
  verbose: 0,
  yieldEvery: 'auto',
});
```

Arguments:

- `model`: an undisposed, already-compiled `LayersModel`.
- `x`: a live input `Tensor`.
- `y`: a live target `Tensor`.
- `epochs`: a positive integer.
- `batch_size`: a positive integer.

The wrapper should reject invalid integer arguments before calling TFJS. It
should let TFJS reject uncompiled models, incompatible tensor shapes, and loss
configuration errors so diagnostics match TFJS behavior.

The return type is `Promise<TrainingHistory>`. Waluau code must use the
existing coroutine/promise pattern:

```lua
local history: TrainingHistory =
    promise.await(tf.layers_model_fit_one(model, xs, ys, 20, 4))::extern::TrainingHistory
```

No compiler or runtime Promise changes are required. A rejected TFJS fit
promise should propagate through the existing async error path used by model
loading and async tensor readback.

The wrapper should set `shuffle: false` for deterministic browser conformance.
Random shuffling, validation splits, sample weights, `initialEpoch`,
`stepsPerEpoch`, and validation tensors are deferred.

## TrainingHistory API

TFJS `fit` resolves to a `History` object whose `history` field stores arrays
of logged values by metric name. The first Waluau readback API should expose
only loss:

```lua
tf.training_history_len(history: TrainingHistory): i32
tf.training_history_loss(history: TrainingHistory, epoch: i32): f64
```

Rules:

- `training_history_len` returns the number of numeric entries in
  `history.history.loss`.
- `training_history_loss` uses zero-based epoch indexing.
- Out-of-range epoch indexes should throw a clear host `RangeError`.
- Missing, non-array, or non-numeric loss history should throw a clear host
  error. This catches accidental callback/history shape changes without
  inventing a generic JS object reader.

Do not add generic `training_history_metric(history, name, epoch)` yet. That
would require stable string-keyed object access and metric-name policy. Add
metric helpers only when metrics are introduced to `compile`.

`TrainingHistory` itself has no explicit disposal function in the first slice.
It is a small JS object that references logged scalar values, not backend tensor
resources. If future callback APIs retain tensors or logs with tensor values,
that decision should be revisited.

## Callback and Yielding Policy

Do not expose training callbacks in the first slice.

TFJS `fit` supports callback objects with hooks such as `onEpochEnd`,
`onBatchEnd`, and `onYield`, and `yieldEvery` can be `'auto'`, `'batch'`,
`'epoch'`, `'never'`, or a millisecond interval. Waluau can represent a single
synchronous callback today, but training callbacks are async-adjacent and can
run many times while TFJS owns the training loop. That needs a separate design
for callback lifetime, thrown errors, cancellation, coroutine interaction, and
log object representation.

The first wrapper should hard-code:

- `verbose: 0`, so TFJS does not depend on console/progress behavior;
- `yieldEvery: 'auto'`, so browser training remains responsive by default;
- no callbacks, so there is no Waluau callback reentrancy during training.

Do not expose `yieldEvery` in the first slice. If tests become flaky because
`'auto'` timing differs by browser backend, the host wrapper may use
`yieldEvery: 'epoch'` for deterministic tests only if the public design is
updated. Avoid `yieldEvery: 'never'` in browser-facing APIs because it can block
the page during longer training runs.

## Disposal and Lifetime Ownership

Inputs and targets are caller-owned tensors. `fit_one` must not dispose `x` or
`y`, even if training fails.

Model weights and optimizer state are model-owned. `tf.dispose_layers_model`
continues to be the public operation that releases model-owned backend
resources. Compiling a model does not transfer ownership to Waluau.

The returned `TrainingHistory` is host-owned ordinary JS data and has no
backend tensor lifetime in this slice. It may be retained by Waluau as an extern
value until the embedding JS garbage collector collects it.

The wrapper should reject:

- disposed model handles;
- disposed input or target tensors;
- non-`LayersModel` values passed as the model;
- non-`Tensor` values passed as `x` or `y`.

`tf.tidy` remains synchronous and should not wrap `fit_one`. TFJS tidy scopes do
not provide an async cleanup boundary for promises. Programs should create
training tensors, await `fit_one`, read history, then explicitly dispose
training tensors and the model.

## Browser Conformance Strategy

Use the existing browser conformance runner and checked-in TFJS fixture
strategy from PR #259.

Recommended conformance fixture:

- a tiny local `LayersModel` fixture with one dense layer and trainable weights,
  or a model constructed by a test-only fixture generation script and committed
  as `model.json` / `weights.bin`;
- small in-memory `TensorData` arrays for `x` and `y`;
- `compile_sgd(model, "meanSquaredError", 0.1)`;
- `fit_one(model, xs, ys, epochs, batch_size)` awaited through
  `promise.await`;
- `training_history_len(history) == epochs`;
- final loss is finite and lower than initial loss;
- prediction after training moves toward the target;
- all caller-owned tensors and the model are explicitly disposed.

Avoid remote model URLs and random initial assertions. If the fixture has
random initialization, the test should use a fixed committed model asset and
assert broad monotonic behavior rather than exact floating point values. Prefer
deterministic weights where possible.

Add a playground preset only after conformance passes. The preset should show a
small training run that updates the DOM with final loss or prediction, and it
must dispose `xs`, `ys`, prediction/output tensors, and the model. It should not
include user-controlled callbacks or long-running training loops.

## Non-Goals

- `layers_model_fit` with general options.
- `fitDataset`, `trainOnBatch`, `evaluate`, validation data, sample weights, or
  validation split.
- Metrics.
- Callback objects, cancellation, progress streaming, or custom losses.
- Optimizer extern objects, optimizer disposal, momentum/Adam/RMSProp, or
  learning-rate schedules.
- Model or layer construction APIs such as `tf.sequential`, `tf.model`,
  `tf.input`, and `tf.layers.*`.
- Model save/export APIs.
- Multi-input, multi-output, named tensor maps, or tensor arrays.
- Automatic tensor disposal.

## Implementation Beads

Create implementation beads for the approved incremental slice only:

1. Add extern/linker declarations and host wrappers for
   `TrainingHistory`, `layers_model_compile_sgd`,
   `layers_model_fit_one`, `training_history_len`, and
   `training_history_loss`.
2. Add browser conformance fixture coverage for deterministic single-input
   `LayersModel` training and history readback.
3. Add a playground preset/demo for a short, explicitly-disposed training run
   after conformance is green.

The first runtime bead can include a small compiler/linker test for namespace
wiring. The conformance/playground bead should own any fixture assets so the
runtime API can land with focused host behavior first.

## Recommendation Summary

The constrained API is worthwhile. It exercises real TFJS training with the
types Waluau already supports, proves `Promise<TrainingHistory>` is enough for a
first async training result, and avoids the callback/options surface that would
force broader JS interop decisions too early.

Ship `compile_sgd` plus `fit_one` first. Reserve the name `fit` for a later API
with explicit options, metrics, callbacks, validation, and multi-input support.
