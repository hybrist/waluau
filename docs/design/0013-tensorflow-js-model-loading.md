# 0013: TensorFlow.js Model Loading and Inference

## Status

Draft.

## Goal

Define the next TensorFlow.js host surface after the tensor arithmetic MVP in
[0012](0012-tensorflow-js-host-surface.md). This note evaluates model loading,
loaded model objects, single-input inference, async execution, and the boundary
between inference and training.

The recommended next slice is intentionally inference-first:

- load URL-backed `GraphModel` and `LayersModel` objects;
- run single-input, single-output inference against existing `Tensor` values;
- dispose model-owned weights explicitly;
- keep multi-input maps, tensor arrays, model construction, layers, compile, and
  training as follow-up work.

This keeps the surface usable for converted TensorFlow/Keras models without
forcing Waluau to solve object literal option bags, arrays of extern tensors,
named tensor maps, callbacks, datasets, optimizer objects, or training history
readback in the same milestone.

References:

- <https://js.tensorflow.org/api/latest/#loadGraphModel>
- <https://js.tensorflow.org/api/latest/#loadLayersModel>
- <https://js.tensorflow.org/api/latest/#tf.GraphModel.predict>
- <https://js.tensorflow.org/api/latest/#tf.GraphModel.execute>
- <https://js.tensorflow.org/api/latest/#tf.LayersModel.predict>
- <https://js.tensorflow.org/api/latest/#tf.LayersModel.compile>
- <https://js.tensorflow.org/api/latest/#tf.LayersModel.fit>

## Current Baseline

The 0012 MVP exposes `Tensor` and `TensorData` as opaque extern host objects,
rank-limited constructors, arithmetic, `matmul`, readback, and explicit
`dispose`/`tidy` lifetime controls through `externs/tfjs.walu`. Browser
conformance and the playground demo already prove:

- `require("tfjs")` resolves to the built-in TFJS extern namespace;
- host calls can pass and return opaque TFJS tensors;
- `Promise<T>` plus `promise.await` works for async tensor readback;
- TFJS host objects must be explicitly disposed in examples and tests.

That baseline is enough to model async model loading as `Promise<ModelType>`.
The existing Promise support is not enough to make async callbacks, dataset
iterators, or training progress APIs pleasant, but it is enough for:

```lua
local model: GraphModel = promise.await(tf.load_graph_model(url))
local output: Tensor = tf.graph_model_predict(model, input)
```

## Model Types

Add separate nominal extern types for graph and layers models:

```lua
type GraphModel = extern
type LayersModel = extern
```

Do not introduce a shared `Model` extern in the first model slice. TFJS
`GraphModel` and `LayersModel` overlap for inference, but diverge around
`execute`, `compile`, `fit`, Keras layers, save/load details, and metadata. A
shared type would either hide useful methods or imply training support on graph
models.

Loaded model values own backend resources through their weights. They should be
treated like `Tensor` host objects:

- Waluau cannot mutate model fields;
- model methods return new `Tensor` handles that callers must dispose;
- model disposal is explicit and separate from tensor disposal;
- using a disposed model is a host runtime error.

## Loading Surface

First model-loading declarations:

```lua
tf.load_graph_model(url: string): Promise<GraphModel>
tf.load_layers_model(url: string): Promise<LayersModel>
```

These map to `tf.loadGraphModel(url)` and `tf.loadLayersModel(url)`.

Only string URL/loading shortcuts are included in the first slice. This includes
ordinary `http://` / `https://` model JSON URLs and TFJS URL-like storage
schemes that the browser host already supports, such as `localstorage://` and
`indexeddb://` where applicable.

Deferred loading options:

- `fromTFHub`, `strict`, `requestInit`, `onProgress`, `weightPathPrefix`,
  `weightUrlConverter`, `streamWeights`, and custom `fetchFunc`;
- `tf.io.IOHandler` objects, `tf.io.browserFiles`, `tf.io.http`, and file
  uploads;
- `loadGraphModelSync`, because the first browser/playground path should not
  require prebuilt in-memory artifacts.

Rationale: Waluau does not yet have ergonomic object literal interop for
structured JS options or a callback surface for progress events. A URL-only
loader lets the runtime validate network/CORS/backend behavior without
inventing a general options representation.

## Inference Surface

First inference declarations should handle exactly one input tensor and one
returned output tensor:

```lua
tf.graph_model_predict(model: GraphModel, input: Tensor): Tensor
tf.graph_model_predict_async(model: GraphModel, input: Tensor): Promise<Tensor>
tf.graph_model_execute(model: GraphModel, input: Tensor): Tensor
tf.layers_model_predict(model: LayersModel, input: Tensor): Tensor
```

`graph_model_predict_async` maps to `GraphModel.predictAsync` and is needed for
models with control-flow ops. `graph_model_execute` maps to `GraphModel.execute`
with default outputs. `LayersModel.predict` is synchronous in TFJS for tensor
inputs and returns a tensor or tensor array; the first Waluau wrapper must reject
array/map results with a clear host error.

Do not expose multi-output or named-output variants until the host surface has
one of these:

- `TensorList` extern helpers for arrays of tensors;
- a `NamedTensorMap` extern helper with string-key lookup;
- broader object/record interop for JavaScript dictionaries.

The host implementation should validate return shape at the boundary:

- If TFJS returns a single tensor, return it as `Tensor`.
- If TFJS returns an array or named map, throw a clear error explaining that the
  first Waluau model API only supports single-output models.

## Model Metadata

Add a minimal diagnostic surface:

```lua
tf.graph_model_input_count(model: GraphModel): i32
tf.graph_model_output_count(model: GraphModel): i32
tf.layers_model_input_count(model: LayersModel): i32
tf.layers_model_output_count(model: LayersModel): i32
```

The counts let tests reject accidental multi-input/multi-output fixtures before
the host call throws. They are also a low-risk way to prove model object access
without committing to layer traversal, symbolic tensors, or full model summary
printing.

Names and full shape metadata are deferred. TFJS exposes names/shapes through
model internals, but strings, optional dimensions, arrays, and nested symbolic
metadata would expand the slice. If needed later, prefer dedicated helper
functions over returning raw JS objects.

## Lifetime Semantics

Add explicit model disposal:

```lua
tf.dispose_graph_model(model: GraphModel): unit
tf.dispose_layers_model(model: LayersModel): unit
```

These map to `model.dispose()`. They release model-owned weights and resources,
not caller-owned input/output tensors. Inference outputs are ordinary `Tensor`
values and follow 0012 tensor lifetime rules.

`tf.tidy` remains synchronous. Do not wrap `load_graph_model`,
`load_layers_model`, or `graph_model_predict_async` in `tf.tidy`; TensorFlow.js
does not use promises as tidy cleanup boundaries. Programs should either:

- load a model outside tidy scopes, dispose it when done, and dispose outputs;
- use synchronous `predict`/`execute` inside `tidy` only when the returned tensor
  is the direct return value or explicitly kept.

## Promise Support Evaluation

Current `Promise<T>` support is sufficient for the recommended loading and async
inference slice:

- `Promise<GraphModel>` and `Promise<LayersModel>` are ordinary generic extern
  specializations;
- `promise.await` already resumes coroutines with typed extern payloads;
- `Promise<Tensor>` from `predictAsync` matches the existing async tensor
  readback pattern.

No compiler/runtime Promise changes are required for model loading.

Current Promise support is not sufficient for a good training surface. TFJS
training relies on callback objects, repeated async yields, history objects,
datasets/iterators, and sometimes cancellation or UI progress. Waluau can call a
minimal `Promise<TrainingHistory>` wrapper, but doing so without callback and
history access would not be useful enough to justify exposing `fit` yet.

## Training Evaluation

Training should be split from inference. TFJS `LayersModel` training requires:

- `compile(args)` with optimizer/loss/metrics strings, functions, or optimizer
  objects;
- `fit(x, y, args)` returning `Promise<History>`;
- optional callbacks such as epoch/batch hooks and `yieldEvery`;
- optional validation tensors, sample weights, dataset support, and metrics;
- mutable model weights and optimizer state.

Do not expose `compile`, `fit`, `evaluate`, `fitDataset`, `trainOnBatch`, layer
construction, `tf.Variable`, or optimizer objects in the first model-loading
slice.

The smallest useful future training slice should be a separate design and
should avoid callback-rich APIs at first:

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

tf.training_history_loss(history: TrainingHistory, epoch: i32): f64
tf.training_history_len(history: TrainingHistory): i32
```

Even this should wait until URL-loaded inference is proven. It should include a
browser conformance fixture using a tiny deterministic layers model and must
define how callbacks, yielding, and history objects are represented before
expanding beyond SGD/string-loss training.

## Non-Goals

- Multi-input or multi-output model calls.
- Named tensor maps or output node selection.
- Model save/export APIs.
- File upload, custom IO handlers, or TF Hub-specific options.
- Keras layer construction, `tf.sequential`, `tf.model`, `tf.input`, or
  `tf.layers.*`.
- Training, evaluation, callbacks, datasets, optimizers, metrics, and history
  readback.
- Automatic model or output tensor disposal.
- Static shape checking for model inputs or outputs.

## Dependency Order

| # | Work | Depends on | Notes |
|--:|------|------------|-------|
| 1 | Extend `externs/tfjs.walu` with `GraphModel` / `LayersModel` and URL loaders | 0012 MVP | Pure extern/linker namespace surface. |
| 2 | Add browser/playground host wrappers for URL loading and model disposal | 1 | Validates TFJS availability, Promise return values, disposed model errors. |
| 3 | Add single-input/single-output inference wrappers | 1, 2 | `predict`, `predictAsync`, and `execute` wrappers validate tensor/model types and reject arrays/maps. |
| 4 | Add conformance fixtures with tiny static model assets | 2, 3 | Prefer local test assets over remote network dependencies. |
| 5 | Add playground preset/demo for async model loading and inference | 2, 3, 4 | Shows `promise.await`, inference output readback, and explicit disposal. |
| 6 | Design minimal training API | 1-5 | Separate design after inference behavior is proven. |

## Test Plan

Compiler/linker tests:

- `GraphModel` and `LayersModel` are distinct nominal extern types.
- `Promise<GraphModel>` and `Promise<LayersModel>` declarations lower like
  existing `Promise<TensorData>` declarations.
- `tfjs` namespace maps new public names to stable host import names.
- Type checking rejects passing `LayersModel` to graph-only helpers and
  `GraphModel` to layers-only helpers.

Runtime/browser conformance:

- Load a tiny local graph model from a fixture URL using `promise.await`.
- Load a tiny local layers model from a fixture URL using `promise.await`.
- Run single-input inference and validate output through `tf.data_sync` or
  async `tf.data`.
- Verify `GraphModel.predictAsync` can be awaited and returns a tensor.
- Verify `GraphModel.execute` default-output behavior for a single-output graph.
- Verify model disposal releases model-owned tensors/resources as visible
  through `tf.memory().numTensors` where the backend reports it.
- Verify a clear runtime error for a fixture that returns multiple outputs, or
  for a host-level mock that returns an array/map.

Playground tests:

- Add a preset that loads a small checked-in model asset, awaits the loader,
  predicts from a constructed tensor, reads the first output value, and disposes
  input/output/model handles.
- Avoid remote model URLs in tests. Network, CORS, CDN availability, and model
  size should not determine CI stability.

## Recommendation

Implement URL-loaded inference before training. The next implementation bead
should add `GraphModel` / `LayersModel`, URL loaders, disposal, and
single-input/single-output inference wrappers. A second bead should add local
model fixtures plus conformance/playground coverage. Training should remain a
separate follow-up design after the model object and Promise boundaries are
proven in CI.
