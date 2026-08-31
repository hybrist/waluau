# Luau conformance deviations and pending inventory

This document separates incomplete Waluau work from tests that intentionally
exercise a different execution or language model. Waluau compiles ahead of time
to Wasm GC, runs in browsers, and uses the DOM as its host interface. The
reference suite also tests Luau VM internals, native embedding hooks, dynamic
source loading, and untyped table behavior that are not part of that contract.

The mappings below are many-to-many. A chunk can exercise more than one
deviation, and a pending chunk can also contain a fixable gap. Split such a
chunk when that exposes an independently meaningful passing assertion.

Bead IDs are repository issue records. Inspect one with `bd show <id> --json`.
The parent inventory is `waluau-q7qg`; its audit records are `waluau-q7qg.1`,
`waluau-q7qg.2`, and `waluau-q7qg.3`.

## Snapshot and reproduction

The snapshot is recomputed whenever this document changes. Count all imported
chunks and pending markers with the two commands in [`README.md`](./README.md).
To verify a compact family, expand its range and check that every named file
exists and still contains the pending directive. For example:

```sh
for n in $(seq 1 25); do
    test -f "conformance/luau/events.$n.walu"
    rg -q '^-- conformance: pending$' "conformance/luau/events.$n.walu"
done
```

After `waluau-h37g` completed the interpolated-string work, the directory has
**1,091 chunks**: **426 enabled** and **665 pending**. These numbers are an exact
filesystem snapshot, not a target. Run
`node conformance/luau/check-pending-inventory.mjs` to verify the counts, every
family mapping below, the compact intentional sets, and the sole exact runner
exclusion.

PR #642 split eight coarse inputs into 79 chunks; later gap fixes and focused
splits can likewise change both the total and the enabled/pending counts. The
meaningful measure remains independently passing upstream coverage. The most
recent change is `waluau-h37g`, which enabled `stringinterp.2` and
`stringinterp.6` and added one patched companion, moving the snapshot from
1,090/423/667 to 1,091/426/665.

## Intentional execution-model inventory

The 2026-08-30 audit and final compiler probe identify this exact current
**153-chunk union**. It includes Luau's experimental `integer` VM extension,
which the initial documentation omitted even though the compiler audit had
classified all 19 chunks with the register/native families. Four native chunks
were enabled by `waluau-q7qg.5`; `native.1` and `native.50` were enabled later.
The remaining chunks are pending for deliberate execution-model or scope
reasons, rather than a small missing parser or library implementation.

| Category | Exact pending chunks | Count |
| --- | --- | ---: |
| [Native/JIT, Luau integer extension, and VM register layout](#nativejit-luau-integer-extension-and-vm-register-layout) | `native.{2-7,10-18,21-45,47-49,51-58}`, `integers.{1-19}`, `integers_regspill.{1-6}`, `native_integer_spills.{1-3}`, `native_types`, `native_userdata` | 81 |
| [Wasm GC observability](#wasm-gc-observability) | `gc.{2-4,6-24}` | 22 |
| [Metatable event model](#metatable-event-model) | `events.{1-25}` | 25 |
| [Native C-yield continuations](#native-c-yield-continuations) | `cyield.{1-13}` | 13 |
| [Embedding and instrumentation hooks](#embedding-instrumentation-and-environments) | `apicalls`, `exceptions`, `coverage`, `debug`, `debugger`, `interrupt`, `iter_fenv`, `ndebug_upvalues`, `safeenv`, `types` | 10 |
| [Reference test userdata](#reference-test-userdata) | `udata_direct`, `userdata` | 2 |

The enabled exceptions in those source families are `native.1`, `native.8`,
`native.9`, `native.19`, `native.20`, `native.46`, `native.50`, `gc.1`, `gc.5`,
and `gc.25`; their independent assertions already pass.

### Native/JIT, Luau integer extension, and VM register layout

Luau's `native` and register-spill tests inspect its bytecode VM, native-code
compiler, integer register behavior, optimization/deoptimization paths, and
test-only helpers such as `noinline` and `is_native`. Waluau emits Wasm GC
directly; it has no Luau VM register file or Luau native/JIT tier for those
assertions to observe. Implementing analogous Waluau optimizations would not
make those VM-specific assertions conformance tests.

`integers.{1-19}` tests Luau's experimental 64-bit `integer` VM value, `123i`
literal suffix, `integer.*` namespace, integer-specific type identity, and
native-tier behavior. Waluau instead exposes ordinary Wasm numeric types such
as `i64` and `u64`; adding a second Luau-VM integer value solely for this suite
is out of scope. This is distinct from ordinary numeric operations and
scientific notation, which are supported and tested elsewhere.

The browser conformance runner supplies imported `luau/*` chunks with a small
authored `is_native_if_supported(): bool` preamble that always returns `false`.
That preserves upstream tests' ordinary fallback paths without introducing a
production builtin or implying an optional native tier. Assertions that require
the probe to become true still fail and remain in this intentional-deviation
category.

Impact: the 81 chunks in the table above remain intentionally pending. Where a
file contains an ordinary language assertion independent of native execution,
it should be split, as `native.1`, `native.46`, and `native.50` demonstrate.

### Wasm GC observability

Object lifetime is managed by the browser's Wasm GC implementation. Waluau does
not promise Luau's `collectgarbage` controls, weak-table behavior, finalization
order, allocation counters, or collection timing. Tests that require forcing a
collection or observing when a key becomes unreachable are therefore not
deterministic browser conformance tests.

Impact: `gc.{2-4,6-24}` (22 chunks) remains pending. `gc.1`, `gc.5`, and
`gc.25` contain independent behavior that does not require those observations
and are enabled.

### Metatable event model

Waluau uses typed arrays and records with statically known fields and operators;
it does not implement Lua/Luau's dynamic metatable event dispatch
(`__index`, `__newindex`, arithmetic/comparison events, `__call`, `__len`, and
related fallback ordering). This is a language-model difference, not a missing
Wasm instruction.

Impact: `events.{1-25}` (25 chunks) remains pending. Other coarse chunks that
mix ordinary assertions with metatable assertions should be split rather than
classified wholesale.

### Native C-yield continuations

The `cyield` source is driven by Luau's native C API harness. It validates
yielding through C callbacks, continuation functions, C stack state, and
resume/error transfer across that boundary. Browser Waluau coroutines suspend
compiled Wasm functions and declared browser async operations; there is no
Luau C stack or `lua_yield` continuation callback surface.

Impact: `cyield.{1-13}` (13 chunks) remains pending. Nine of them also make
explicit coroutine-library calls and therefore appear in the coroutine set
below: `cyield.{1-4,6,10-13}`.

### Embedding, instrumentation, and environments

Several upstream files are programs for a companion native harness rather than
self-contained language tests:

- `apicalls` and `exceptions` are invoked through Luau's native embedding API.
- `coverage`, `debug`, `debugger`, `interrupt`, `ndebug_upvalues`, and `types`
  inspect VM frames, coverage counters, breakpoints, interrupt callbacks,
  upvalue metadata, or test RTTI.
- `iter_fenv` and `safeenv` mutate or inspect a Lua global environment. Waluau
  resolves names and imports ahead of time and has no mutable `_G`/`getfenv`/
  `setfenv` execution environment.

Impact: those ten singleton chunks remain pending. Browser tooling may expose
source maps or application diagnostics, but that does not provide the Luau VM
hooks asserted by these tests.

### Reference test userdata

`udata_direct` and `userdata` depend on host-defined C++ test values such as
`vec2`, `vertex`, and `int64`, including their mutable slots and operator
metamethods. Waluau's browser host boundary is declared through DOM externs;
these Luau test-host constructors are not part of it.

Impact: the two singleton chunks remain pending. `native_userdata` also uses
those values, but is already counted in the native/JIT family because its
purpose is native-code generation over test userdata.

## Broad language and API deviations

These sets can overlap the 153-chunk execution-model union above. They are
listed independently because their semantic impact reaches otherwise ordinary
language/library families.

### Typed coroutine API

Waluau deliberately gives coroutines a typed, asymmetric API:

- `coroutine.create` accepts a zero-argument function returning `i32`; callers
  capture initial arguments in its closure.
- `coroutine.resume` accepts one `thread` and returns either
  `(bool, unknown)` or a typed `Yielded(unknown) | Finished(i32) | Error(string)`
  result, depending on context. It does not forward arbitrary resume arguments.
- `coroutine.yield` carries one `unknown` value; it does not yield an arbitrary
  result list.
- `coroutine.close` returns `bool`.
- Luau's `wrap`, `status`, `running`, and `isyieldable` observation APIs are not
  part of the Waluau coroutine surface. Closures and typed result variants make
  lifecycle state explicit without exposing the reference VM's thread API.

This avoids a first-resume/subsequent-resume argument asymmetry that would
require an effect or session type throughout every function that can yield.
Promise suspension remains browser-shaped through `promise.await` and the
compiler's coroutine lowering.

Exact impact: **71 pending chunks** explicitly exercise the differing API:

- `coroutine.{1-22}` (22)
- `cyield.{1-4,6,10-13}` (9)
- `pcall.{10-18,20-23,27,33-36,39,41-46,52-59,63-65,67-68}` (38)
- `errors.51`, `errors.74` (2)

Within that set, `coroutine.wrap` appears in 29 chunks, `status` in 27,
`running` in five, and `isyieldable` in three; those API counts overlap. Some
chunks also require protected-call improvements, so future splits can still
enable their non-coroutine assertions. The settled API design is recorded in
bead `waluau-nar`.

### Strict boolean control flow

Waluau requires `bool` conditions and defines `and`, `or`, and `not` over
booleans. It does not apply Lua/Luau truthiness to nil, numbers, strings,
records, or `unknown`, and `and`/`or` do not return an arbitrary selected
operand. This keeps control flow statically typed and makes nullable handling
explicit.

Exact impact in the provenance-split `ifelseexpr` source is three pending
chunks: `ifelseexpr.1`, `ifelseexpr.3`, and `ifelseexpr.9`. PR #646 enabled the
remaining six chunks after implementing the independent parser gaps. Other
large sources also contain truthiness checks; keep those intervals pending or
split them from ordinary boolean assertions rather than treating truthiness as
an incomplete parser feature.

### Static lexical names and module environments

Waluau resolves lexical names, module imports, and declared browser-host
symbols ahead of time. Reading an undeclared name is a compile error; it does
not silently produce `nil`, and compiled modules do not expose a mutable `_G`
environment that can replace resolved builtins. This is the same static module
contract that makes `getfenv`/`setfenv` and per-loaded-chunk environments
inapplicable, but it also affects ordinary expressions such as the deliberate
undefined-name interpolation in `stringinterp.4`.

Impact: chunks whose only intended behavior is undeclared-name-to-`nil` or
runtime global-environment substitution remain pending. Coarse chunks that also
contain a fixable parser, inference, or library gap are mapped to both reasons
in the family inventory below; this deviation is not a blanket explanation for
unknown-name diagnostics on missing standard-library namespaces.

`stringinterp.4` is the exact case: it interpolates `localName`, a name the
upstream test never declares, and expects the text `nil`. Waluau reports
`unknown name 'localName'` at compile time. The decision is deliberate and not
scheduled work — resolving the name dynamically would require a mutable global
environment, which the static module contract rules out — so the chunk stays
pending permanently rather than under a bead.

### Reserved primitive type keywords

Waluau's primitive type names — `string`, `number`, `bool`, `unknown`, `nil`,
`thread`, `bytes`, `unit`/`void`, and the sized numerics `i32`/`i64`/`u32`/
`u64`/`f32`/`f64` — are lexer keywords, not ordinary identifiers. A local, a
parameter, a function name, or a record field name therefore cannot be spelled
`string`. In expression position `string` denotes the builtin `string.*`
namespace, which the compiler resolves statically by name (the same static
resolution described above), so a binding could not shadow it even if the
keyword were relaxed.

Impact: `stringinterp.8` is pending because upstream deliberately names a
function parameter `string` to test that a local shadows the `string` library.
The interpolation semantics that range exercises — one untyped parameter
interpolated after being called with both a string and a number argument —
already work, and `stringinterp.8.patched.walu` keeps that coverage enabled
with the parameter renamed. Only the shadowing itself is out of contract.

### Ahead-of-time compilation and `loadstring`

Waluau compiles the module graph before browser instantiation. It does not ship
the Rust compiler inside the produced Wasm module and does not evaluate source
strings at runtime. Consequently `loadstring`, generated-code diagnostics,
per-loaded-chunk environments, and tests that synthesize functions from source
are outside the runtime language contract.

Exact impact: **78 pending chunks** contain explicit `loadstring` use:

- `basic.28`; `calls.{14-15}`; `closure.{11,21}`
- `constructs.{39-46,53}`
- `errors.{1-16,21,27,32-33,41-49,55-72,75-78}`
- `gc.4`; `literals.{1,5,7}`; `locals.2`; `math.15`
- `pm.{88-89}`; `sort`; `strings.129`; `tables.1`; `utf8.2`; `vararg`

`basic.2.patched` mentions `loadstring` only in adaptation comments and is
enabled; it is not included in the 78. A chunk with a direct assertion that is
independent of generated source should be split.

### Binary packing versus browser text strings

Luau `string.pack`, `string.unpack`, and `string.packsize` treat strings as
arbitrary byte sequences. Waluau strings use the browser's text-string model;
raw binary data is represented separately as `bytes`, typed-array views, or
mutable `buffer` values. Adding binary packing that returns a text string would
conflate those contracts.

Impact: `tpack.{1-19}` (19 chunks) remains pending as a deliberate API-shape
difference. Buffer string conversion is a deliberately narrow bridge rather
than a redefinition of every string: `buffer.fromstring`/`writestring` accept
only browser string code units U+0000..U+00FF and map each to one byte;
`buffer.tostring`/`readstring` project every byte back to the same code unit.
Embedded NUL and all 256 byte values therefore round-trip, while wider Unicode
input is rejected catchably. Use immutable `bytes` when browser text projection
is not part of the operation.

### Mutable buffers

Waluau now implements Luau's fixed-size mutable `buffer` value over browser
Wasm linear memory: zero-based scalar access, binary string projection, bulk
copy/fill, and bit-field access all have browser conformance coverage. The
1-GiB allocation and 6-GiBit offset path is part of enabled `buffers.20`; it no
longer needs a pending resource-stress carve-out in the routine browser suite.

The imported source is represented by **24 current chunks**, of which **22 are
enabled**: `buffers.{1-20}`, `buffers.8_bulk_bounds`, and
`buffers.20_small_bitops`. The two remaining pending chunks fail for reasons
outside the buffer API:

- `buffers.18_untyped_table` passes an unannotated table parameter. Waluau
  keeps that parameter `unknown`, so `t16[index]` and `#t16` are rejected rather
  than refined from the call site. This broad unknown-indexing/static-typing
  gap is tracked by `waluau-2dow`; it does not justify adding sparse or mixed
  Lua tables.
- `buffers.21` repeats the buffer assertions from the preceding source ranges
  under a final `getfenv()` call whose purpose is to force Luau's VM slow-call
  paths. Waluau resolves environments ahead of time and exposes no mutable
  `getfenv` execution environment, and this aggregate adds no unique buffer
  semantics. It therefore remains an intentional VM/environment deviation.

### Sparse, mixed, and hash tables

Waluau separates contiguous homogeneous arrays from statically shaped records.
It does not provide one Lua table value that can simultaneously contain an
array part, arbitrary hash keys, record fields, and interior nil holes. Array
length and iteration therefore do not implement Lua's sparse-table boundary
rules, and records are not traversed with `pairs`/`next`.

The seven coarse sources from the audit now map to these current pending chunks
after provenance splitting:

| Upstream source | Current pending chunks or span containing the excluded checks |
| --- | --- |
| `attrib.luau` | `attrib.4` (upstream lines 24-117) |
| `basic.luau` | `basic.22` (214-292), `basic.24` (296-383), `basic.26` (385-391), `basic.28` (393-775), and `basic.34` (40-198 in its source-ordered split sequence) |
| `clear.luau` | `clear` (whole source) |
| `locals.luau` | `locals.2` (31-138; the hash/query-table subsection is 122-138) |
| `move.luau` | `move` (whole source) |
| `tables.luau` | `tables.1` (the remaining coarse table-layout source) |
| `vararg.luau` | `vararg` (whole source; mixed `{ n = count, ... }` packs and nil holes) |

That audited mapping is **11 current chunks**. Two additional complete chunks
were independently browser-probed and remain pending for the same scope
decision: `iter.13` requires an interior nil hole, and `bitwise.9` wraps every
one of its assertions in `pairs(c)` over an authored array, which Waluau
iterates directly. `bitwise.9` is a single loop, so it has no upstream
assertion boundary to split on. These chunks commonly contain other blockers;
split out passing contiguous ranges, but do not implement sparse/mixed/hash
behavior under this conformance epic.

### Dense-array `ipairs`

Waluau supports `ipairs(array)` as a compile-time special form only in the
iterator position of a generic `for`. It evaluates the array expression once
and yields 1-based indices followed by the corresponding values from a dense,
contiguous array. A call returning multiple values is adjusted to its first
result before iteration, as in Luau.

This support does not make the builtin a first-class iterator factory: manual
`local inext = ipairs(t)` triples are not provided, and `getfenv` cannot replace
the builtin. Interior nil holes and mixed/hash table parts remain excluded by
the table model above. An authored lexical binding named `ipairs` still behaves
as an ordinary iterator factory and shadows the special form.

The implementation enables exact upstream chunks
`basic.22.ipairs_dense` and `basic.22.ipairs_multret`. Every other pending
chunk containing an `ipairs` call was browser-probed and remains pending for an
independent reason:

| Pending chunk(s) | Independent blocker after dense `ipairs` support |
| --- | --- |
| `basic.22` | nil-hole/manual-iterator checks plus sparse, mixed, and hash-table assertions |
| `basic.28`, `basic.34` | broader VM/metatable/environment cases and array `pairs` checks surrounding the `ipairs` assertions |
| `iter.9` | the array contains an interior nil hole |
| `iter_fenv` | runtime environment substitution through `getfenv` |
| `calls.{33,34,35,37,38,39}` | recursive dynamic `unpack`/multi-value helpers and their individual call-shape assertions |
| `tpack.{11,12}` | binary `string.pack`/`string.unpack` APIs |
| `closure.12` | coroutine status/yield/resume behavior and dynamic varargs |
| `clear`, `coverage` | hash-table clearing/iteration and VM coverage instrumentation, respectively |
| `native.39` | native/JIT-focused iterator-protocol coverage |

## Exhaustive pending-family mapping

The browser probe covers every pending file, while this table assigns every
current filename stem to at least one documented deviation or **open** bead.
Counts sum to all **665 pending chunks**. The `trackedByFamily` data in
`check-pending-inventory.mjs` is the machine-checked form of this table: an
unknown family, changed count, missing mapping, or stale compact set fails
`./check`. A family mapping names its primary blockers; individual coarse
chunks can contain more than one and should still be split when a contiguous
assertion range becomes independently useful.

| Pending filename family | Count | Primary deviation or open bead |
| --- | ---: | --- |
| `apicalls*` | 1 | [Embedding hooks](#embedding-instrumentation-and-environments) |
| `assert*` | 1 | [Strict booleans](#strict-boolean-control-flow); dynamic/special builtin calls `waluau-9f8d` |
| `attrib*` | 2 | [Sparse/mixed/hash tables](#sparse-mixed-and-hash-tables) |
| `basic*` | 14 | Sparse tables, strict booleans, [AOT source loading](#ahead-of-time-compilation-and-loadstring), [static names](#static-lexical-names-and-module-environments); dynamic calls/inference `waluau-9f8d` |
| `bitwise*` | 5 | Array `pairs` under the table model; string-number coercion `waluau-dbyy`, protected invalid arguments `waluau-rndq`, `waluau-esz6`, uninitialized-local inference `waluau-3em1` |
| `buffers*` | 2 | Unknown array refinement `waluau-2dow`; `getfenv`/VM slow-call aggregate is an embedding deviation |
| `calls*` | 43 | AOT source loading and sparse tables; multi-value/runtime-vararg work `waluau-jnyd`, `waluau-zxju`, `waluau-n6u8`; dynamic calls/inference `waluau-9f8d` |
| `classes*` | 49 | Luau class declarations `waluau-wll8` |
| `clear*` | 1 | Sparse/mixed/hash tables |
| `closure*` | 19 | Typed coroutines, AOT source loading, sparse tables, static names; dynamic calls/inference `waluau-9f8d` |
| `constructs*` | 41 | Strict booleans, AOT source loading, sparse tables; dynamic calls/inference `waluau-9f8d` |
| `coroutine*` | 22 | [Typed coroutine API](#typed-coroutine-api) |
| `coverage*` | 1 | Embedding/instrumentation hooks |
| `cyield*` | 13 | [Native C-yield continuations](#native-c-yield-continuations) and typed coroutines |
| `datetime*` | 1 | Browser `os.date`/`os.time` compatibility `waluau-qabb` |
| `debug*`, `debugger*` | 2 | Embedding/instrumentation hooks |
| `errors*` | 80 | AOT source loading and sparse tables; `xpcall` `waluau-wb7a`, error formatting `waluau-fg46`, dynamic calls/inference `waluau-9f8d` |
| `events*` | 25 | [Metatable event model](#metatable-event-model) |
| `exceptions*` | 1 | Embedding/instrumentation hooks |
| `explicit_type_instantiations*` | 1 | Explicit generic instantiation/type packs `waluau-9ttd` |
| `gc*` | 22 | [Wasm GC observability](#wasm-gc-observability) |
| `ifelseexpr*` | 3 | Strict boolean control flow |
| `integers*` | 19 | [Luau experimental integer VM extension](#nativejit-luau-integer-extension-and-vm-register-layout) |
| `integers_regspill*` | 6 | Native/JIT and VM register layout |
| `interrupt*` | 1 | Embedding/instrumentation hooks |
| `iter*` | 31 | Sparse tables, typed coroutines, and metatable events; unpack/varargs `waluau-zxju`, `waluau-n6u8`; record `pairs` `waluau-yfus` |
| `iter_fenv*` | 1 | Embedding/environment hooks |
| `literals*` | 4 | AOT source loading and sparse tables; dynamic builtin calls `waluau-9f8d` |
| `locals*` | 1 | AOT source loading and sparse/mixed/hash tables |
| `math*` | 7 | AOT source loading; unknown refinement `waluau-2dow`, protected/multi-results `waluau-8fxn`, `waluau-jnyd`, `waluau-n6u8` |
| `move*` | 1 | Sparse/mixed/hash tables |
| `native*` | 51 | Native/JIT and VM register layout |
| `native_integer_spills*` | 3 | Native/JIT and VM register layout |
| `native_types*` | 1 | Native/JIT type-observation harness |
| `native_userdata*` | 1 | Native/JIT plus reference test userdata |
| `ndebug_upvalues*` | 1 | Embedding/instrumentation hooks |
| `pcall*` | 66 | Typed coroutines; multi-success `waluau-8fxn`, `xpcall` `waluau-wb7a`, protected builtins `waluau-esz6`, dynamic calls/inference `waluau-9f8d` |
| `pm*` | 52 | AOT source loading and sparse tables; pattern coercion/replacements `waluau-dbyy`, protected calls `waluau-esz6`, `waluau-wb7a` |
| `safeenv*` | 1 | Embedding/environment hooks |
| `sort*` | 1 | AOT source loading and sparse/mixed/hash tables |
| `stringinterp*` | 2 | [Static lexical names](#static-lexical-names-and-module-environments) (`stringinterp.4`) and [reserved primitive type keywords](#reserved-primitive-type-keywords) (`stringinterp.8`) |
| `strings*` | 25 | AOT source loading, sparse tables, metatable events; coercion `waluau-dbyy`, catchable formatting `waluau-nlyf`, byte ranges `waluau-vogb`, error formatting `waluau-fg46`, dynamic calls `waluau-9f8d` |
| `tables*` | 4 | AOT source loading and sparse/mixed/hash tables; runtime unpack `waluau-zxju` |
| `tmerror*` | 1 | Metatable event model |
| `tpack*` | 19 | [Binary packing/text-string difference](#binary-packing-versus-browser-text-strings) |
| `types*` | 1 | Embedding/test RTTI hooks |
| `udata_direct*`, `userdata*` | 2 | [Reference test userdata](#reference-test-userdata) |
| `utf8*` | 1 | AOT source loading plus runtime unpack `waluau-zxju` |
| `vararg*` | 1 | Sparse/mixed packs; remaining vararg/unpack work `waluau-n6u8`, `waluau-zxju` |
| `vector*`, `vector_library*` | 12 | Vector value/library `waluau-uneu` |

## Fixable gaps remain tracked work

The categories above must not become a blanket excuse for unrelated failures.
The final probe links bounded implementation work wherever Waluau intends to
converge:

| Open gap | Bead | Current mapped impact |
| --- | --- | --- |
| Unknown-typed array refinement | `waluau-2dow` | `buffers.18_untyped_table` and dynamic helper portions of `math*`; this does not authorize sparse/mixed tables |
| Protected calls and multi-results | `waluau-8fxn`, `waluau-wb7a`, `waluau-esz6` | `pcall.*`, `errors.*`, and pattern chunks not blocked solely by coroutine deviations |
| Multi-value call spreading and runtime unpack | `waluau-jnyd`, `waluau-zxju`, `waluau-n6u8` | Vararg/unpack call sites that do not require sparse packs |
| Dynamic calls and recursive inference | `waluau-9f8d` | Ordinary `assert`, `basic`, `calls`, `closure`, `constructs`, `errors`, `literals`, `pcall`, and `strings` chunks after intentional blockers are split away |
| String/number coercion and pattern replacements | `waluau-dbyy` | Remaining `pm.*` cases after protected-call and parser gaps, plus `bitwise.{14,15}` |
| Protected invalid `bit32` arguments | `waluau-rndq`, `waluau-esz6` | `bitwise.22` |
| Uninitialized-local inference | `waluau-3em1` | `bitwise.18` |
| Luau class declarations | `waluau-wll8` | `classes.{1-49}` |
| Browser date/time library | `waluau-qabb` | `datetime` |
| Explicit generic instantiation/type packs | `waluau-9ttd` | `explicit_type_instantiations` |
| Vector value/library | `waluau-uneu` | `vector`, `vector_library.{1-11}`; native vector checks remain VM/JIT exclusions |

`bitwise.{14,15}` were re-probed under `waluau-rndq` and deliberately left to
`waluau-dbyy`. Luau's `bit32` accepts `"1"` because its VM converts strings to
numbers for every numeric argument, and Waluau rejects that conversion
everywhere: `"1" + 1` does not compile either. Adding it at `bit32` argument
positions alone would give one library a coercion rule the language does not
have, and `waluau-dbyy` already tracks the uniform string/number rule that
would cover `bit32`, arithmetic, and pattern captures together. `tonumber` is
the explicit conversion until then.

Completed work is reflected rather than left in the open-gap table: mutable
buffers have 22 enabled chunks; deterministic typed math and exact
`math.noise` ranges are enabled; builtin functions as values landed in
`waluau-390t`; dense-array `ipairs` landed in `waluau-uxuf`; fixed
multi-results through nested vararg calls enabled `pcall.1` and `pcall.4` in
`waluau-sdc0`. Shortest-round-trip numeric `tostring` landed in `waluau-9wf5`
and enabled the last seven `strconv` chunks, so that family no longer appears
above. Luau's modulo-2^32 `bit32` argument rule for wide, negative, and
`unknown`-typed operands enabled `bitwise.{2,3,6,17,20,21,23}` in
`waluau-rndq`. Earlier children also enabled scientific notation, surplus
fixed-call arguments, bit32 intrinsics, large `%f` formatting, missing
multi-binding nil padding, chained if expressions, and many passing split
ranges. `waluau-h37g` closed the interpolation work: `table.concat` now joins
numeric arrays through the same `tostring` formatting Luau uses, `\ ` lexes as
an escaped space, and the two remaining `stringinterp` chunks are documented
language decisions rather than gaps.
When another fix lands, recompile the affected pending chunks, split
independent sections where necessary, and update this inventory rather than
preserving an obsolete failure reason.
