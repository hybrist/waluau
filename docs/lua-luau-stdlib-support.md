# Lua / Luau Standard Library Support

## Status

Living document — tracks how much of the Lua 5.4 and Luau standard library
Waluau currently implements, and where Waluau exposes something with the same
name or spelling but **different behaviour**.

Last reviewed: 2026-06-13.

## How to read this document

Waluau is *Lua-like in syntax, not Lua-compatible in semantics* (see
[`docs/design/0002-language-v0.md`](design/0002-language-v0.md)). It is a
statically typed, ahead-of-time compiler targeting Wasm. As a result:

- There is **no runtime stdlib written in Lua**. Everything below is a
  compiler builtin that is type-checked in the HIR and lowered to Wasm
  instructions or host imports.
- Builtins are recognised by their fully-qualified name (`math.abs`,
  `string.find`, …). They are **not** values: you cannot store `math.abs` in a
  local, pass it as a callback, or iterate `math` as a table.
- Anything not listed as *Supported* or *Different* is **not implemented** and
  will fail name resolution / type checking.

Legend:

- ✅ **Supported** — present and behaves like Lua/Luau (modulo static typing).
- ⚠️ **Different** — same name exists but observable behaviour differs from
  Lua/Luau. Read the notes.
- ❌ **Not supported** — not implemented; using it is a compile error.

The authoritative source for builtin signatures is
[`crates/waluau-hir/src/builtins.rs`](../crates/waluau-hir/src/builtins.rs);
lowering lives in [`crates/waluau-ir/src/lower.rs`](../crates/waluau-ir/src/lower.rs).
Behaviour is pinned by the conformance suite under
[`conformance/`](../conformance).

---

## Cross-cutting differences

These affect the whole library surface and are the most common source of
surprise for people coming from Lua/Luau.

| Area | Lua / Luau | Waluau | Notes |
|------|------------|--------|-------|
| Number type | single `number` (f64) + 64-bit integers (5.4) | distinct scalar types `i32`, `i64`, `u32`, `u64`, `f32`, `f64` | `number` is accepted only as an **alias for `f64`**. Builtins are typed against specific scalar types. |
| Array indexing | 1-based | **0-based** | `xs[0]` is the first element. See `conformance/arrays.walu`. |
| Truthiness | every value except `nil`/`false` is truthy | conditions must be `bool` | No implicit truthiness; `assert`, `if`, `while` require `bool`. |
| `nil` | first-class value | not a general value | Absence is modelled with nullable extern types / tagged unions, not `nil`. |
| Multiple returns | nil-terminated, dynamic | static `(T1, T2, …)` tuple types | Functions declare a fixed multi-value return type. |
| Generic `for` iterator protocol | `for vars in f, s, ctl` stops on first `nil` | `for vars in arr` iterates arrays directly; `for vars in f` calls `f` returning `(bool continue, values…)` and stops when the bool is `false` | See `conformance/for_in.walu`. Not the Lua iterator triple. |
| Metatables / `__index` etc. | full metatable protocol | ❌ none | No `setmetatable`, `getmetatable`, `rawget`, `rawset`, `rawequal`, `rawlen`. |
| `::` operator | Luau type **assertion** (`expr :: T`) | repurposed as a **numeric cast** | Only valid between numeric scalar types; performs an actual conversion, not an assertion. |
| Compound assignment | Luau has `+= -= *= /= //= %= ^= ..=` | all of `+= -= *= /= //= %= ^= ..=` | Arithmetic forms require a numeric target; `..=` requires a string target. The target is evaluated once. See `conformance/compound_assignment.walu`. |

---

## Basic / global functions

| Function | Status | Behaviour in Waluau / difference from Lua/Luau |
|----------|--------|------------------------------------------------|
| `print` | ⚠️ Different | Signature is `print(message: string): unit`. **String-only and single-argument** — not variadic, does not accept arbitrary values, does not insert tabs or a trailing newline contract. Lowered to a host `print` import. |
| `tostring` | ⚠️ Different | Accepts only **primitive** inputs: numeric, `bool`, or `string`. No tables, functions, metatable `__tostring`, or `nil`. Each numeric width maps to a dedicated host conversion import. |
| `assert` | ⚠️ Different | Requires **exactly one `bool`** argument. No optional message, no passthrough of truthy values, and it does **not return** its argument. A false assert traps at runtime. |
| `tonumber` | ❌ | Not implemented (use `::` casts between numeric types). |
| `type` | ❌ | Not implemented (types are static, not runtime-queryable). |
| `error` / `pcall` / `xpcall` | ⚠️ Partial | `error(string)`, `assert(cond, string)`, and scalar `pcall(f, ...) -> (bool, unknown)` use Wasm exception handling. `xpcall`, multi-value success payloads, non-bool assert truthiness, and raw-trap recovery are not yet supported. |
| `select` | ⚠️ Partial | `select('#', ...)` returns the vararg count; `select(n, ...)` returns the **single** value at 1-based position `n` (negative `n` counts from the end) rather than the whole tail. Out-of-range positions trap. See `conformance/select_n.walu`. |
| `next` / `pairs` / `ipairs` | ❌ | Not implemented; iterate with the generic `for` protocol above. |
| `rawget` / `rawset` / `rawequal` / `rawlen` | ❌ | No metatable/raw access. |
| `setmetatable` / `getmetatable` | ❌ | No metatables. |
| `unpack` / `table.unpack` | ❌ | Not implemented. |
| `require` | ⚠️ Planned/partial | Module system is specified in [`design/0008-require-modules.md`](design/0008-require-modules.md); see that doc for current state rather than Lua's `package`/`require` runtime semantics. |
| `collectgarbage`, `load`, `loadstring`, `dofile`, `loadfile`, `rawequal`, `getfenv`/`setfenv` | ❌ | No dynamic loading or environment manipulation. |
| `_G`, `_VERSION` | ❌ | No global environment table. |

## Operators that stand in for stdlib functions

| Operator | Status | Notes |
|----------|--------|-------|
| `..` (concat) | ✅ (strings) | `string .. string → string`. Numeric operands are **not** auto-coerced (Lua coerces numbers); use `tostring` first. Backed by `wasm:js-string` `concat`. |
| `#` (length) | ✅ | Works on strings (host `length`, UTF-16 code units on JS hosts) and arrays. |
| `==`, `<`, `>`, `<=`, `>=` on strings | ✅ | Value/lexicographic comparison via `wasm:js-string` `equals`/`compare`. |
| `^` (exponentiation) | ✅ | Computes in floating point like Lua (operands widened to `f64`, backed by a host `math_pow` import). Binds tighter than the unary operators and is right-associative. The result is converted back to the operand type, so `i32 ^ i32 → i32` (truncated), matching how `/` keeps the operand type. See `conformance/exponentiation.walu`. |

---

## `string` library

Strings are immutable host-managed `externref` values (see
[`design/0010-strings-m3.md`](design/0010-strings-m3.md)). Methods may be
called as `string.fn(s, …)` or `s:fn(…)`.

| Symbol | Status | Behaviour / difference |
|--------|--------|------------------------|
| `string.find` / `s:find` | ✅ | Lua-conforming signature `string.find(s, pattern, init?, plain?)`: returns the 1-based inclusive `start, end` (plus captures for literal patterns), or `nil` when there is no match. Full Lua pattern support, including classes, anchors, quantifiers, captures, position captures, `%b`, `%f`, and back-references. `init` may be negative; `plain` selects plain substring search. See `conformance/string_find.walu` and `conformance/luau/pm.*.walu`. |
| `string.sub` / `s:sub` | ✅ | Signature `string.sub(value, first, last?)`. Indices are Lua-style **1-based** and `last` is inclusive. `last` defaults to the end of the string; negative indices count back from the end. |
| `string.len` / `s:len` / `#s` | ✅ | Returns string length as `i32`. |
| `string.format` / `s:format` | ⚠️ Partial | Supports up to 100 values with width, precision, flags, `%c`, `%d`/`%i`, `%u`, `%o`, `%f`, `%g`/`%G`, `%x`/`%X`, `%s`, `%q`, Luau `%*`, and `%%`. |
| `string.rep` / `s:rep` | ✅ | Signature `string.rep(value, count, separator?)`; `separator` defaults to `""`. |
| `string.lower` / `string.upper` and methods | ✅ | Host string case conversion. |
| `string.byte` / `s:byte` | ⚠️ Partial | Signature `string.byte(value, index?, last?)`. Indices are Lua-style **1-based** (negative counts from the end) and ranges can return statically-sized multiple values. Scalar out-of-range still returns `-1` instead of `nil`. |
| `string.char` | ⚠️ Partial | Supports 0 to 16 statically expanded `i32` byte values and returns the corresponding string. |
| `string.match` / `s:match` | ✅ | Lua-conforming: returns the pattern's captures (or the whole match), each `nil` on no match. Capture arity and types are derived statically from literal patterns; non-literal patterns are treated as capture-free (the whole match is returned). |
| `string.gsub` / `s:gsub` | ⚠️ Partial | Lua-conforming `string.gsub(s, pattern, repl, n?)` returning `result, count`. `repl` may be a template string (`%0`–`%9`, `%%`) or a function of the captures returning a string, a number, or nothing (keep the match). Table replacements are not supported. |
| `string.gmatch` / `s:gmatch` | ⚠️ Partial | Supported as a `for ... in string.gmatch(s, p)` iterator; loop variables bind the captures (or the whole match). Not usable as a first-class iterator value. |
| `string.split` (Luau) | ❌ | Not implemented. |
| `string.reverse` | ✅ | Returns the reversed string. |
| Pattern matching (Lua patterns) | ✅ | Full Lua pattern engine (a port of Luau's `lstrlib.c` matcher) backing `string.find`/`match`/`gmatch`/`gsub`, including `%b`, `%f`, back-references, position captures, and Lua-style malformed-pattern errors. Patterns operate per UTF-16 code unit with ASCII character classes; `%z` matches the zero code unit. |

## `table` library

| Symbol | Status | Behaviour / difference |
|--------|--------|------------------------|
| `table.concat` | ⚠️ Different | Signature `table.concat(list, sep?)`. Requires `list` to be an **array of `string`** (`{string}`); does **not** stringify numbers and has no `i`/`j` range arguments. Returns a `string`. |
| `table.insert` / `table.remove` | ⚠️ Different | Implemented on growable arrays with **0-based** positions (`table.insert(t, v)`, `table.insert(t, pos, v)`, `table.remove(t, pos?)`). |
| `table.sort` | ⚠️ Different | Implemented (default `<` ordering; comparator arguments not supported). |
| `table.pack` | ⚠️ Different | Returns a fresh `{unknown}` array of the packed values; the count is available as both `.n` and `#`. Indexing is **0-based** (`t[0]..t[t.n - 1]`). See `conformance/table_pack.walu`. |
| `table.unpack` | ❌ | Not implemented (runtime-variadic multi-value returns; tracked separately). |
| `table.create` / `table.clone` / `table.find` (Luau) | ❌ | Not implemented. |
| `table.move`, `table.freeze`, `table.isfrozen` | ❌ | Not implemented. |

## `math` library

Math builtins lower **directly to Wasm float instructions**, so they operate on
and return **floating-point** values only (`f32`/`f64`). This is the biggest
semantic gap from Lua/Luau.

| Symbol | Status | Behaviour / difference |
|--------|--------|------------------------|
| `math.abs` | ⚠️ Different | **Float-only** (`f32`/`f64`). Lua/Luau also accept integers; here an integer argument is a type error. |
| `math.sqrt` | ✅ | Float-only (matches Lua, which returns a float). |
| `math.floor` | ⚠️ Different | Returns a **float**, not an integer (Lua 5.4/Luau return an integer/`number` you can use as an index). Float-only input. |
| `math.ceil` | ⚠️ Different | Same as `math.floor` — returns a float. |
| `math.min` / `math.max` | ⚠️ Different | Exactly **two** arguments, both the **same float type** (Lua is variadic and integer-capable). |
| `math.trunc` | ⚠️ Non-standard | Not a Lua function (closest Lua equivalent is `math.modf`). Maps to Wasm `trunc`. Float-only. |
| `math.nearest` | ⚠️ Non-standard | Not a Lua/Luau function. Round-to-nearest-even via Wasm `nearest` (Luau's `math.round` rounds half away from zero — different). |
| `math.copysign` | ⚠️ Non-standard | Not a Lua function. Maps to Wasm `copysign`. Two float args. |
| `math.pi` | ✅ | Compile-time `f64` constant (folds to a literal; no host import). |
| `math.huge`, `math.maxinteger`, `math.mininteger` | ❌ | Not implemented. |
| `math.random` / `math.randomseed` | ❌ | Not implemented. |
| `math.fmod`, `math.modf`, `math.pow`, `math.log`, `math.exp`, `math.sin/cos/tan`, … | ❌ | Not implemented. |
| `math.clamp`, `math.sign`, `math.round`, `math.noise`, `math.lerp` (Luau) | ❌ | Not implemented. |

## `coroutine` library

Coroutines are supported but use a **typed, tagged-union** result model rather
than Lua's dynamic multiple-return convention (see
[`design/0007-coroutines.md`](design/0007-coroutines.md) and
`conformance/coroutines*.walu`).

| Symbol | Status | Behaviour / difference |
|--------|--------|------------------------|
| `coroutine.create` | ⚠️ Different | Argument must be a **zero-argument, `i32`-returning function**, not an arbitrary function. Returns a `thread`. |
| `coroutine.resume` | ⚠️ Different | Returns a **tagged union** `Error(string) \| Finished(i32) \| Yielded(unknown)` (a `(bool, unknown)` shape is also accepted by inference). Lua returns `true, …` / `false, err`. |
| `coroutine.yield` | ⚠️ Different | Takes **exactly one** value (typed `unknown`); returns `unit`. Lua's yield is variadic and returns the resume arguments. |
| `coroutine.close` | ⚠️ Different | Returns a `bool` rather than Lua's `true` / `false, err` pair. |
| `coroutine.await_promise` | ⚠️ Non-standard | Waluau extension: awaits an extern `Promise`-like value, yielding `unknown`. Not part of Lua/Luau. |
| `coroutine.wrap` | ❌ | Not implemented. |
| `coroutine.status` / `coroutine.running` / `coroutine.isyieldable` | ❌ | Not implemented. |

### `promise` (Waluau extension — not Lua/Luau)

| Symbol | Status | Notes |
|--------|--------|-------|
| `promise.await` / `p:await()` | ⚠️ Non-standard | Awaits a `Promise<T>` extern and resolves to `T`. Host/JS interop feature, no Lua analogue. See `conformance/promise_await.walu`. |

## Other standard libraries

| Library | Status | Notes |
|---------|--------|-------|
| `os` (`os.time`, `os.clock`, `os.date`, …) | ❌ | Not implemented. |
| `io` (`io.write`, `io.read`, file handles) | ❌ | Not implemented; `print` is the only output path. |
| `debug` | ❌ | Not implemented. |
| `package` / module loading | ⚠️ | Covered by the `require`/modules design, not Lua's `package` runtime. |
| `utf8` | ❌ | Not implemented (string length/indexing are host-defined). |
| `bit32` (Lua 5.2 / Luau) | ❌ | Not implemented; use native integer types and operators where available. |
| `buffer` (Luau) | ⚠️ Partial | Binary data is handled by Waluau's separate **`bytes`** type, not Luau's `buffer` API. See `conformance/bytes.walu`. |

---

## Beyond the Lua/Luau stdlib

Waluau also exposes a large **host/DOM extern surface** (browser APIs, `fetch`,
storage, events, promises) generated into [`externs/dom.walu`](../externs/dom.walu).
These are **not** part of the Lua or Luau standard library and are intentionally
out of scope for this tracking document; they are documented under
[`design/0011-dom-api-support.md`](design/0011-dom-api-support.md).

## Updating this document

When you add, change, or remove a builtin:

1. Update the relevant row here and adjust its status icon.
2. Add or update a focused `conformance/*.walu` test that pins the behaviour.
3. If the behaviour diverges from Lua/Luau, say so explicitly in the *notes*
   column — the "behaves differently" cases are the whole point of this file.
