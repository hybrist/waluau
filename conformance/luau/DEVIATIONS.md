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

After the exact `math.noise` split in `waluau-q7qg.11.2`, the directory has
**1,085 chunks**: **377 enabled** and **708 pending**. These numbers are a
snapshot, not a target.
PR #642 split eight coarse inputs into 79 chunks; that raised the raw chunk
count while exposing 42 enabled upstream slices. PRs #646 and #647 subsequently
added one focused chunk and enabled five more upstream chunks.

## Intentional execution-model inventory

The 2026-08-30 audit initially identified a 140-chunk union. Four native-family
chunks were subsequently enabled by the bit32 work in `waluau-q7qg.5`, leaving
this exact current **136-chunk union**. These chunks are pending for deliberate
execution-model or scope reasons, rather than a small missing parser or library
implementation.

| Category | Exact pending chunks | Count |
| --- | --- | ---: |
| [Native/JIT and VM register layout](#nativejit-and-vm-register-layout) | `native.{1-7,10-18,21-45,47-58}`, `integers_regspill.{1-6}`, `native_integer_spills.{1-3}`, `native_types`, `native_userdata` | 64 |
| [Wasm GC observability](#wasm-gc-observability) | `gc.{2-4,6-24}` | 22 |
| [Metatable event model](#metatable-event-model) | `events.{1-25}` | 25 |
| [Native C-yield continuations](#native-c-yield-continuations) | `cyield.{1-13}` | 13 |
| [Embedding and instrumentation hooks](#embedding-instrumentation-and-environments) | `apicalls`, `exceptions`, `coverage`, `debug`, `debugger`, `interrupt`, `iter_fenv`, `ndebug_upvalues`, `safeenv`, `types` | 10 |
| [Reference test userdata](#reference-test-userdata) | `udata_direct`, `userdata` | 2 |

The enabled exceptions in those source families are `native.8`, `native.9`,
`native.19`, `native.20`, `native.46`, `gc.1`, `gc.5`, and `gc.25`; their
independent assertions already pass.

### Native/JIT and VM register layout

Luau's `native` and register-spill tests inspect its bytecode VM, native-code
compiler, integer register behavior, optimization/deoptimization paths, and
test-only helpers such as `noinline` and `is_native`. Waluau emits Wasm GC
directly; it has no Luau VM register file or Luau native/JIT tier for those
assertions to observe. Implementing analogous Waluau optimizations would not
make those VM-specific assertions conformance tests.

The browser conformance runner supplies imported `luau/*` chunks with a small
authored `is_native_if_supported(): bool` preamble that always returns `false`.
That preserves upstream tests' ordinary fallback paths without introducing a
production builtin or implying an optional native tier. Assertions that require
the probe to become true still fail and remain in this intentional-deviation
category.

Impact: the 64 chunks in the table above remain intentionally pending. Where a
file contains an ordinary language assertion independent of native execution,
it should be split, as `native.46` already demonstrates.

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

These sets can overlap the 136-chunk execution-model union above. They are
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
were independently browser-probed during the compiler audit: `pcall.1` traps
because `table.pack` is a mixed array plus `n` field, and `iter.13` requires an
interior nil hole. They remain pending for the same scope decision. These
chunks commonly contain other blockers; split out passing contiguous ranges,
but do not implement sparse/mixed/hash behavior under this conformance epic.

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

## Fixable gaps remain tracked work

The categories above must not become a blanket excuse for unrelated failures.
The audits linked bounded implementation work where Waluau intends to converge:

| Gap | Bead | Current impact |
| --- | --- | --- |
| Remaining mutable-buffer slow-call/resource coverage | `waluau-q7qg.6.6` | Ordinary-size bit operations pass in `buffers.20_small_bitops`; `buffers.20` retains protected error and 1-GiB resource slices, while intentionally pending VM/environment aggregate `buffers.21` remains out of scope. `buffers.8`, `.9`, and `.11` carry their separately tracked dynamic-f32 and untyped-numeric blockers |
| Typed math-library completion | `waluau-q7qg.11` | `math.{1,4.helper,9,15,17}`, `math.2.coercion`, `math.11.numeric`, and `math.17.multivalue`; direct scalar and exact-noise ranges are enabled, while the aggregate remains pending for its named dynamic, protected-call, multi-value, and source-loading blockers |
| Protected calls and multi-results | `waluau-8fxn`, `waluau-wb7a`, `waluau-esz6` | `pcall.*`, `errors.*`, and pattern chunks not blocked solely by coroutine deviations |
| Multi-value call spreading and runtime unpack | `waluau-jnyd`, `waluau-zxju`, `waluau-n6u8` | Vararg/unpack call sites that do not require sparse packs |
| Builtin functions as values | `waluau-390t` | Higher-order library checks outside native harness families |
| Dense-array `ipairs` (completed) | `waluau-uxuf` | Exact dense assertions are enabled as `basic.22.ipairs_dense` and `basic.22.ipairs_multret`; the remaining occurrence audit is classified above |
| String/number coercion and pattern replacements | `waluau-dbyy` | Remaining `pm.*` cases after protected-call and parser gaps |

Completed children of `waluau-q7qg` already enabled scientific notation,
surplus fixed-call arguments, bit32 intrinsics, large `%f` formatting, missing
multi-binding nil padding, chained if expressions, and many passing split
ranges. When a fix lands, recompile the affected pending chunks, split
independent sections where necessary, and update this inventory rather than
preserving an obsolete failure reason.
