# Luau conformance deviations and pending inventory

This document separates incomplete Waluau work from tests that intentionally
exercise a different execution or language model. Waluau compiles ahead of time
to Wasm GC, runs in browsers, and uses the DOM as its host interface. The
reference suite also tests Luau VM internals, native embedding hooks, dynamic
source loading, and untyped table behavior that are not part of that contract.

Bead IDs are repository issue records. Inspect one with `bd show <id> --json`.

## Three markers, three meanings

Every imported chunk carries exactly one state:

| Marker | Meaning | Runner |
| --- | --- | --- |
| none | Enabled. The chunk compiles and passes in the browser. | must pass |
| `-- conformance: pending` | A bounded gap someone could fix. Every pending family names the open beads that own it. | must currently fail |
| `-- conformance: out-of-scope: <slug>[,<slug>]` | A documented deliberate difference blocks it. Nobody is expected to make it pass. | must currently fail |

Both non-enabled states are **inverse-tested**: the browser suite fails if such
a chunk starts passing, so neither marker can go stale. They differ only in what
the number means. The pending count is the addressable backlog; the out-of-scope
count is coverage Waluau has decided not to buy.

Each slug in an `out-of-scope` directive names a section below and is validated
by [`check-pending-inventory.mjs`](./check-pending-inventory.mjs). A chunk may
name more than one when several deviations independently block it.

Reproduce the directory-level counts from the repository root with:

```sh
find conformance/luau -maxdepth 1 -name '*.walu' -print | wc -l
rg -l '^-- conformance: pending$' conformance/luau -g '*.walu' | wc -l
rg -l '^-- conformance: out-of-scope:' conformance/luau -g '*.walu' | wc -l
```

List the exact chunk set behind one deviation with, for example:

```sh
rg -l 'out-of-scope:.*aot-loadstring' conformance/luau -g '*.walu'
```

## Snapshot

The directory holds **1,098 chunks**: **441 enabled**, **235 pending**, and
**422 out of scope**. These are an exact filesystem snapshot, not a target.
`node conformance/luau/check-pending-inventory.mjs` verifies them, checks that
every chunk carries exactly one marker, checks every named slug against the
documented set, checks that every pending family names an open bead, and
verifies the two generated tables below. Run it with `--write` to regenerate
those tables after reclassifying a chunk.

Splitting can increase both the total and the non-enabled counts: one coarse
chunk may become several focused chunks plus newly enabled ones. The meaningful
progress metric is enabled upstream coverage, not a monotonically decreasing
pending count.

The classification was rebuilt in `waluau-gwyp` by compiling all 657 previously
pending chunks through the CLI with the browser preamble and reading the first
diagnostic of each, rather than by inheriting the previous per-family
attribution. That probe corrected several stale attributions; the largest is
that most of `native.luau` does not assert the native tier at all. Only
`integers_regspill.{1-6}`, `native.{43,44,45,51,52,53,54}` and `native_types`
call `is_native()`; the rest of that source is ordinary Luau, much of it fuzzer
regressions, so 24 `native.*` chunks are now tracked as bounded gaps.

## Out-of-scope inventory

Generated from the chunk markers. Do not edit by hand; run
`node conformance/luau/check-pending-inventory.mjs --write`.

<!-- generated:out-of-scope -->

| Deviation | Out-of-scope chunks | Families |
| --- | ---: | --- |
| `typed-coroutine` | 82 | `closure`, `coroutine`, `cyield`, `errors`, `gc`, `iter`, `pcall` |
| `aot-loadstring` | 56 | `basic`, `calls`, `constructs`, `errors`, `gc`, `literals`, `locals`, `math`, `pm`, `strings`, `utf8` |
| `static-names` | 50 | `basic`, `buffers`, `calls`, `closure`, `coroutine`, `cyield`, `errors`, `events`, `gc`, `iter`, `math`, `native`, `pcall`, `stringinterp` |
| `sparse-mixed-hash-tables` | 45 | `attrib`, `basic`, `bitwise`, `clear`, `closure`, `constructs`, `events`, `gc`, `iter`, `literals`, `locals`, `move`, `native`, `native_integer_spills`, `pm`, `sort`, `tables`, `vararg` |
| `metatable-events` | 42 | `calls`, `closure`, `coroutine`, `errors`, `events`, `gc`, `iter`, `native`, `pcall`, `pm`, `strings`, `tmerror` |
| `strict-bool` | 33 | `basic`, `closure`, `constructs`, `ifelseexpr`, `native`, `pcall` |
| `luau-integer-vm-extension` | 25 | `integers`, `integers_regspill` |
| `binary-packing` | 19 | `tpack` |
| `static-type-errors` | 18 | `basic`, `errors`, `events`, `pcall` |
| `native-jit-register-layout` | 15 | `integers_regspill`, `native`, `native_types`, `native_userdata` |
| `native-c-yield` | 13 | `cyield` |
| `reference-test-userdata` | 13 | `basic`, `errors`, `events`, `gc`, `iter`, `native_userdata`, `strings`, `udata_direct`, `userdata` |
| `embedding-hooks` | 12 | `apicalls`, `coverage`, `debug`, `debugger`, `exceptions`, `interrupt`, `iter`, `iter_fenv`, `ndebug_upvalues`, `pcall`, `safeenv`, `types` |
| `wasm-gc-observability` | 9 | `gc` |
| `heterogeneous-values` | 7 | `calls`, `constructs` |
| `browser-clocks-and-calendars` | 6 | `datetime` |
| `reserved-type-keywords` | 1 | `stringinterp` |

A chunk may name more than one deviation, so these counts sum to more than the 422 out-of-scope chunks. List the exact set for one deviation with `rg -l 'out-of-scope:.*<slug>' conformance/luau`.

<!-- /generated:out-of-scope -->

## Pending inventory

Generated from the chunk markers and the bead attribution in
[`check-pending-inventory.mjs`](./check-pending-inventory.mjs).

<!-- generated:pending -->

| Pending family | Chunks | Open beads |
| --- | ---: | --- |
| `classes*` | 49 | `waluau-wll8` |
| `pm*` | 33 | `waluau-zxju`, `waluau-lz2e`, `waluau-dbyy`, `waluau-esz6`, `waluau-4487`, `waluau-274e`, `waluau-j74d` |
| `calls*` | 32 | `waluau-j74d`, `waluau-jehg`, `waluau-zxju`, `waluau-2dow`, `waluau-lz2e`, `waluau-9f8d` |
| `native*` | 24 | `waluau-j74d`, `waluau-pndm`, `waluau-31kg`, `waluau-9ttd`, `waluau-uneu`, `waluau-2dow`, `waluau-nsp4`, `waluau-rndq`, `waluau-esz6`, `waluau-9f8d` |
| `pcall*` | 23 | `waluau-wb7a`, `waluau-zxju`, `waluau-jehg`, `waluau-n6u8`, `waluau-274e` |
| `strings*` | 22 | `waluau-j74d`, `waluau-esz6`, `waluau-nlyf`, `waluau-vogb`, `waluau-nsp4`, `waluau-dbyy`, `waluau-9f8d` |
| `vector_library*` | 11 | `waluau-uneu` |
| `basic*` | 5 | `waluau-jehg`, `waluau-pndm`, `waluau-n6u8`, `waluau-9f8d` |
| `constructs*` | 5 | `waluau-jehg`, `waluau-9f8d` |
| `errors*` | 5 | `waluau-wb7a`, `waluau-jehg`, `waluau-844l` |
| `iter*` | 5 | `waluau-j74d`, `waluau-dbyy`, `waluau-3em1`, `waluau-n6u8` |
| `bitwise*` | 4 | `waluau-dbyy`, `waluau-esz6`, `waluau-rndq`, `waluau-3em1` |
| `closure*` | 4 | `waluau-j74d`, `waluau-9f8d` |
| `math*` | 4 | `waluau-dbyy`, `waluau-jehg`, `waluau-n6u8`, `waluau-9f8d` |
| `tables*` | 2 | `waluau-jehg`, `waluau-9f8d` |
| `assert*` | 1 | `waluau-9f8d` |
| `attrib*` | 1 | `waluau-zxju` |
| `buffers*` | 1 | `waluau-2dow` |
| `datetime*` | 1 | `waluau-31kg`, `waluau-9f8d` |
| `explicit_type_instantiations*` | 1 | `waluau-9ttd` |
| `native_integer_spills*` | 1 | `waluau-3em1` |
| `vector*` | 1 | `waluau-uneu` |

Total: 235 pending chunks in 22 families.

<!-- /generated:pending -->

## Documented deviations

Each heading is one `out-of-scope` slug.

### `native-jit-register-layout`

Luau's `native` and register-spill tests inspect its bytecode VM, native-code
compiler, integer register behavior, optimization/deoptimization paths, and
test-only helpers such as `is_native`. Waluau emits Wasm GC directly; it has no
Luau VM register file or Luau native/JIT tier for those assertions to observe.
Implementing analogous Waluau optimizations would not make those VM-specific
assertions conformance tests.

The browser conformance runner supplies imported `luau/*` chunks with a small
authored `is_native_if_supported(): bool` preamble that always returns `false`.
That preserves upstream tests' ordinary fallback paths without introducing a
production builtin or implying an optional native tier. Assertions that require
the probe to become true still fail and stay in this category. A locally defined
`noinline` helper is ordinary Luau and is **not** a native-tier probe; several
chunks that define one are enabled.

`native.53` is additionally excluded by exact name from the browser runner: its
enormous register-spill expression takes roughly 90 seconds to compile once
dynamic numeric inference succeeds, only to reach an irrelevant `is_native`
check. It is the sole exact-name runner exclusion.

### `luau-integer-vm-extension`

`integers.*` and `integers_regspill.*` test Luau's experimental 64-bit `integer`
VM value, the `123i` literal suffix, the `integer.*` namespace, integer-specific
type identity, and native-tier behavior over integer registers. Waluau instead
exposes ordinary Wasm numeric types such as `i64` and `u64`; adding a second
Luau-VM integer value solely for this suite is out of scope. This is distinct
from ordinary numeric operations and scientific notation, which are supported
and tested elsewhere.

### `wasm-gc-observability`

Object lifetime is managed by the browser's Wasm GC implementation. Waluau does
not promise Luau's `collectgarbage` controls, weak-table behavior, finalization
order, allocation counters, or collection timing. Tests that require forcing a
collection or observing when a key becomes unreachable are therefore not
deterministic browser conformance tests. `gc.1`, `gc.5`, and `gc.25` contain
independent behavior that does not require those observations and are enabled.

### `metatable-events`

Waluau uses typed arrays and records with statically known fields and operators;
it does not implement Lua/Luau's dynamic metatable event dispatch (`__index`,
`__newindex`, arithmetic/comparison events, `__call`, `__len`, and related
fallback ordering), nor the raw accessors that exist to bypass it
(`rawget`, `rawset`, `rawequal`, `rawlen`). This is a language-model difference,
not a missing Wasm instruction. Coarse chunks that mix ordinary assertions with
metatable assertions should be split rather than classified wholesale.

`waluau-4l1` is an open P4 backlog note from before the table model settled that
contemplates adding metatables "only after the table model is stable". It
predates this deviation and does not make these chunks scheduled work; if that
design is ever revisited, this section is what has to change first.

### `native-c-yield`

The `cyield` source is driven by Luau's native C API harness. Its thirteen
chunks call C functions such as `passthroughCall`, `pcallThenCall`,
`singleYield`, and `multipleYields`, and validate yielding through C callbacks,
continuation functions, C stack state, and resume/error transfer across that
boundary. Browser Waluau coroutines suspend compiled Wasm functions and declared
browser async operations; there is no Luau C stack or `lua_yield` continuation
callback surface. Seven of the thirteen also exercise the typed coroutine API
and name both slugs; `cyield.10` additionally reads the harness global
`_G.limitedstack`.

### `embedding-hooks`

Several upstream files are programs for a companion native harness rather than
self-contained language tests:

- `apicalls` and `exceptions` are invoked through Luau's native embedding API.
- `coverage`, `debug`, `debugger`, `interrupt`, `ndebug_upvalues`, and `types`
  inspect VM frames, coverage counters, breakpoints, interrupt callbacks,
  upvalue metadata, or test RTTI.
- `iter_fenv` and `safeenv` mutate or inspect a Lua global environment.
- `pcall.40` and `iter.27` call the harness values `cxxthrow` and
  `cYieldingIterator` from otherwise ordinary sources.

Browser tooling may expose source maps or application diagnostics, but that does
not provide the Luau VM hooks these tests assert.

### `reference-test-userdata`

`udata_direct`, `userdata`, and a scattering of chunks in other sources depend
on host-defined C++ test values such as `vec2`, `vertex`, `int64`, and
`newproxy`, including their mutable slots and operator metamethods. Waluau's
browser host boundary is declared through DOM externs; these Luau test-host
constructors are not part of it.

### `typed-coroutine`

Waluau deliberately gives coroutines a typed, asymmetric API:

- `coroutine.create` accepts a zero-argument function returning `i32`; callers
  capture initial arguments in its closure.
- `coroutine.resume` accepts one `thread` and returns either `(bool, unknown)`
  or a typed `Yielded(unknown) | Finished(i32) | Error(string)` result,
  depending on context. It does not forward arbitrary resume arguments.
- `coroutine.yield` carries one `unknown` value; it does not yield an arbitrary
  result list.
- `coroutine.close` returns `bool`.
- Luau's `wrap`, `status`, `running`, and `isyieldable` observation APIs are not
  part of the Waluau coroutine surface. Closures and typed result variants make
  lifecycle state explicit without exposing the reference VM's thread API.

This avoids a first-resume/subsequent-resume argument asymmetry that would
require an effect or session type throughout every function that can yield.
Promise suspension remains browser-shaped through `promise.await` and the
compiler's coroutine lowering. The settled API design is recorded in bead
`waluau-nar`.

### `strict-bool`

Waluau requires `bool` conditions and defines `and`, `or`, and `not` over
booleans. It does not apply Lua/Luau truthiness to nil, numbers, strings,
records, or `unknown`, and `and`/`or` do not return an arbitrary selected
operand. This keeps control flow statically typed and makes nullable handling
explicit.

The one value-producing form is nil-coalescing `or`: when the left operand's
type admits nil -- `T?`, and since `waluau-tg4g` also the top type `unknown` --
`a or b` yields `a` when it is present and `b` when it is nil. That is a nil
test, not a truthiness test, so a present `false` or `0` is returned rather than
replaced, which is where Lua's `or` would differ. `bool?` is rejected outright
because the two readings coincide there.
`conformance/unknown_nil_coalescing.walu` pins the `false` case.

### `static-names`

Waluau resolves lexical names, module imports, and declared browser-host symbols
ahead of time. Reading an undeclared name is a compile error
(`unknown name 'x'`); it does not silently produce `nil`. Assigning to an
undeclared name from a nested scope is a compile error too
(`unknown local 'x'` / `unknown lexical binding 'x'`) rather than creating a
global, and compiled modules do not expose a mutable `_G` environment that can
replace resolved builtins. This is the same static module contract that makes
`getfenv`/`setfenv` and per-loaded-chunk environments inapplicable.

Three shapes dominate the affected chunks: upstream harness globals that no
chunk declares, such as `_G.limitedstack`; Lua's implicit-global assignment from
inside a function or block; and deliberate reads of a name the test never
declares, such as the `localName` interpolation in `stringinterp.4` and the
precedence check in `math.9`.

This is not a blanket explanation for unknown-name diagnostics on missing
standard-library namespaces: a chunk that fails on `string.pack` or
`table.clone` is classified by what is missing, not by the diagnostic's shape.

### `reserved-type-keywords`

Waluau's primitive type names -- `string`, `number`, `bool`, `unknown`, `nil`,
`thread`, `bytes`, `unit`/`void`, and the sized numerics `i32`/`i64`/`u32`/
`u64`/`f32`/`f64` -- are lexer keywords, not ordinary identifiers. A local, a
parameter, a function name, or a record field name therefore cannot be spelled
`string`. In expression position `string` denotes the builtin `string.*`
namespace, which the compiler resolves statically by name, so a binding could
not shadow it even if the keyword were relaxed.

`stringinterp.8` is the exact case: upstream deliberately names a function
parameter `string` to test that a local shadows the `string` library. The
interpolation semantics that range exercises already work, and
`stringinterp.8.patched.walu` keeps that coverage enabled with the parameter
renamed. Only the shadowing itself is out of contract.

### `aot-loadstring`

Waluau compiles the module graph before browser instantiation. It does not ship
the Rust compiler inside the produced Wasm module and does not evaluate source
strings at runtime. Consequently `loadstring`, generated-code diagnostics,
per-loaded-chunk environments, and tests that synthesize functions from source
are outside the runtime language contract. Upstream's `dostring` helper is a
`loadstring` wrapper and counts here.

`basic.2.patched` mentions `loadstring` only in adaptation comments and is
enabled. A chunk with a direct assertion that is independent of generated source
should be split.

### `binary-packing`

Luau `string.pack`, `string.unpack`, and `string.packsize` treat strings as
arbitrary byte sequences. Waluau strings use the browser's text-string model;
raw binary data is represented separately as `bytes`, typed-array views, or
mutable `buffer` values. Adding binary packing that returns a text string would
conflate those contracts.

Buffer string conversion is a deliberately narrow bridge rather than a
redefinition of every string: `buffer.fromstring`/`writestring` accept only
browser string code units U+0000..U+00FF and map each to one byte;
`buffer.tostring`/`readstring` project every byte back to the same code unit.
Embedded NUL and all 256 byte values therefore round-trip, while wider Unicode
input is rejected catchably. Use immutable `bytes` when browser text projection
is not part of the operation.

### `browser-clocks-and-calendars`

Waluau's `os` library is the date/time subset only, declared in
[`builtins/os.walu`](../../builtins/os.walu) and implemented over the browser's
`Date` and `performance` clocks: `os.time(): f64`, `os.difftime`, `os.clock`,
and `os.date(format [, time]): string` with Luau's specifier set
`%aAbBcdHIjmMpSUwWxXyYzZ` and `%%`.

A leading `!` selects UTC and is the reproducible form; without it the browser's
local timezone applies. `conformance/os_date_time.walu` therefore asserts exact
values only for `!` formats.

Four differences from Luau follow from the browser host boundary:

- **No calendar-table values.** A declared host import carries primitives, not
  records, and a Waluau function has one return type. So `os.time` takes no
  arguments and `os.date` always returns a formatted string; `os.date("*t")`
  raises instead of silently formatting the literal text.
- **Pre-epoch instants format normally**, because the browser's `Date`
  represents them: `os.date("!%Y", -1)` is "1969". Luau answers nil below the
  epoch, so Waluau's result type is a plain `string`.
- **No daylight-saving model.** Nothing reports an `isdst` flag.
- **No process, filesystem, locale, or environment entry points.** `os.exit`,
  `os.setlocale`, `os.getenv`, and `os.remove` have no browser meaning and are
  absent rather than stubbed.

`datetime.1` is enabled and covers the `os.date` string formats and the
no-argument `os.time`. `datetime.2` and `datetime.8` compile and raise at
runtime on `os.date("*t")` and `os.time(table)`; the browser suite is what
classifies those two. `datetime.6` is *not* here: its blocker is using an
overload set as a value, which is tracked work.

### `sparse-mixed-hash-tables`

Waluau separates contiguous homogeneous arrays from statically shaped records.
It does not provide one Lua table value that can simultaneously contain an array
part, arbitrary hash keys, record fields, and interior nil holes. Array length
and iteration therefore do not implement Lua's sparse-table boundary rules, and
records are not traversed with `pairs`/`next`.

Concretely, the chunks here are blocked by a mixed or keyed table literal
(`{10, 20; x = "10"}`, `{[1+2] = 4}`, `{6, 9, 7, [4.5] = 11}`), an interior nil
hole (`{1, 2, 3, nil, 5}` in `iter.9` and `iter.13`, which compile and fail in
the browser), a non-integer or sparse index assignment (`a['a'..i] = 0`,
`t[{}] = i`, `GLOBAL_LIST[0] = ...`), a value that carries both record fields and
array positions (`t.n = t.n + 1; t[t.n] = w`), `pairs` over an array, or the
hash-table clearing and cloning APIs `table.clear`/`table.clone`.

A **dense** `local t = {}` followed by `t[i] = v` for contiguous `i` is *not*
this deviation. That is the untyped-array inference gap `waluau-j74d`, and those
chunks are pending.

### `static-type-errors`

Waluau reports at compile time the type errors Lua raises at runtime. A test
that writes a statically ill-typed expression and then catches and compares its
runtime message cannot compile at all, so it cannot pass.

The shape is unmistakable in the affected chunks, which all wrap the expression
in a protected call whose result is compared against an exact Lua message:

```lua
assert(ecall(function() return nil + 5 end) == "attempt to perform arithmetic (add) on nil and number")
assert(ecall(function() for i = 1, 'a' do end end) == "invalid 'for' limit (number expected, got string)")
assert(ecall(function() local t = {} t[nil] = 2 end) == "table index is nil")
```

Making these pass would mean compiling an operation the type checker has already
proved wrong and deferring it to a trap, which is the opposite of what a typed
language buys. This is narrow on purpose: it covers only expressions whose
operand types are statically known to be wrong. Waluau does have runtime type
errors, for values typed `unknown`, and protected calls over those are ordinary
enabled coverage.

### `heterogeneous-values`

A Waluau value has one type, and a Waluau function has one result type. Three
Lua idioms therefore have no Waluau spelling:

- **Return branches must join to one type.** `constructs.28` and `constructs.29`
  return `'a'`, `'b'`, `'c'` and `8` from four branches of the same function.
- **A recursive function's returns must join to one arity and type.**
  `constructs.21`, `constructs.22`, and `constructs.23` mix `return i, 'jojo'`,
  `return i, f(i-1)`, and falling off the end in one function.
- **An array literal has one element type** unless it is written as `{unknown}`.
  `calls.34` and `calls.35` build `{1, 2, 3, 4, false, 10, 'alo', false, assert}`
  and then pack and unpack it.

Widening these would either erase the static types the language exists to
provide or require every such value to be boxed as `unknown` implicitly. A
program that genuinely wants a mixed collection writes `{unknown}` and narrows
on read. This deviation is deliberately narrow: a recursive function whose
result *arity* varies at runtime but whose element type is uniform, such as
upstream's `range` and `unlpack` helpers, is the tracked gap `waluau-zxju`, not
this.

## Related engineering notes

### Mutable buffers

Waluau implements Luau's fixed-size mutable `buffer` value over browser Wasm
linear memory: zero-based scalar access, binary string projection, bulk
copy/fill, and bit-field access all have browser conformance coverage. The
imported source is represented by **24 chunks**, of which **22 are enabled**:
`buffers.{1-20}`, `buffers.8_bulk_bounds`, and `buffers.20_small_bitops`. The
two remaining chunks fail for reasons outside the buffer API:
`buffers.18_untyped_table` passes an unannotated table parameter whose element
*assignment* is unsupported (`waluau-2dow`), and `buffers.21` repeats the buffer
assertions under a final `getfenv()` call whose purpose is to force Luau's VM
slow-call paths, so it is out of scope under `static-names`.

### Dense-array `ipairs`

Waluau supports `ipairs(array)` as a compile-time special form only in the
iterator position of a generic `for`. It evaluates the array expression once and
yields 1-based indices followed by the corresponding values from a dense,
contiguous array. A call returning multiple values is adjusted to its first
result before iteration, as in Luau.

This does not make the builtin a first-class iterator factory: manual
`local inext = ipairs(t)` triples are not provided, and `getfenv` cannot replace
the builtin. Interior nil holes and mixed/hash table parts remain excluded by the
table model above. An authored lexical binding named `ipairs` still behaves as an
ordinary iterator factory and shadows the special form. The implementation
enables `basic.22.ipairs_dense` and `basic.22.ipairs_multret`.

## Fixable gaps remain tracked work

The deviations above must not become a blanket excuse for unrelated failures.
Every pending chunk sits under one of these open beads; the generated pending
table maps each family to the subset that owns it.

| Open gap | Bead |
| --- | --- |
| Empty/untyped table literal element-type inference | `waluau-j74d` |
| Parser and lexer gaps in vendored Luau statement forms | `waluau-jehg` |
| Multi-assignment targets other than plain names | `waluau-pndm` |
| Overloaded builtin resolution for untyped arguments | `waluau-31kg` |
| `tostring` over buffer, unit, and record values | `waluau-nsp4` |
| `error()` with a non-string value | `waluau-844l` |
| Two compiler internal errors (`pm.76`, `pcall.47`) | `waluau-274e` |
| Unknown-typed array refinement | `waluau-2dow` |
| Protected calls and multi-results | `waluau-d480`, `waluau-wb7a`, `waluau-esz6` |
| Multi-value spreading and runtime-variadic returns/unpack | `waluau-zxju`, `waluau-n6u8` |
| Dynamic calls and recursive inference | `waluau-9f8d` |
| String/number coercion and pattern replacements | `waluau-dbyy` |
| Protected invalid `bit32` arguments | `waluau-rndq` |
| Uninitialized-local inference | `waluau-3em1` |
| Omitted trailing argument for an unannotated parameter | `waluau-lz2e` |
| Catchable `string.format` failures | `waluau-nlyf` |
| Range-form `string.byte` on dynamic strings | `waluau-vogb` |
| Luau class declarations | `waluau-wll8` |
| Explicit generic instantiation and type packs | `waluau-9ttd` |
| Vector value and library | `waluau-uneu` |
| `pm.106` multi-value lowering mismatch | `waluau-4487` |

`bitwise.{14,15}` were re-probed under `waluau-rndq` and deliberately left to
`waluau-dbyy`. Luau's `bit32` accepts `"1"` because its VM converts strings to
numbers for every numeric argument, and Waluau rejects that conversion
everywhere: `"1" + 1` does not compile either. Adding it at `bit32` argument
positions alone would give one library a coercion rule the language does not
have. `tonumber` is the explicit conversion until then.

Completed work is reflected rather than left in the table above: mutable buffers
have 22 enabled chunks; deterministic typed math and exact `math.noise` ranges
are enabled; builtin functions as values landed in `waluau-390t`, extended to
compiler intrinsics in `waluau-9f8d.2`; dense-array `ipairs` landed in
`waluau-uxuf`; fixed multi-results through nested vararg calls landed in
`waluau-sdc0`; seeded fixpoint inference for recursive top-level functions landed
in `waluau-9f8d.1`; shortest-round-trip numeric `tostring` landed in
`waluau-9wf5`; Luau's modulo-2^32 `bit32` argument rule landed in `waluau-rndq`;
multi-value protected-call success payloads landed in `waluau-8fxn`; the
interpolation work closed in `waluau-h37g`; and the browser `os` date/time
surface landed in `waluau-qabb`. When another fix lands, recompile the affected
chunks, split independent sections where necessary, rerun
`check-pending-inventory.mjs --write`, and update this inventory rather than
preserving an obsolete failure reason.
