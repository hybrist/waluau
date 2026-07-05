# 0016 — Varargs, `table.pack`, `select`, and dynamic `unknown` operations

Status: implemented (waluau-4l3, waluau-y29v).

## Vararg representation

A vararg function (`function f(a, b, ...)`) carries an implicit trailing
parameter of type `{unknown}` at the IR/wasm level. Inside the function, the
expression `...` denotes that array. Values flow through varargs boxed but
otherwise unchanged (numbers stay numbers, tables keep identity); the old
call-site stringification of numeric/bool vararg arguments was removed —
`tostring(unknown)` unboxes at runtime instead.

### Call sites

For a call to a vararg function with fixed parameter count `f`:

- Explicit arguments fill the fixed parameters first; leftovers are boxed
  into a fresh `{unknown}` tail array.
- A **trailing** `...` forwards the caller's varargs:
  - With `k >= f` explicit arguments, the tail is the extra explicit values
    followed by a copy of the forwarded varargs (an inline append loop).
  - With `k < f`, the missing fixed parameters are read from the front of
    the forwarded varargs (`ArrayGet`, which traps when too few values were
    passed — Lua would see `nil`) and the rest are passed along via
    `ArraySlice`.
- `...` anywhere but the last argument is a compile error
  (`'...' is only supported as the last argument of a call`). Lua would
  truncate a non-final `...` to one value; that adjustment is future work.

### Table constructors

A trailing `...` in an array-style table literal splats the varargs:
`{...}` is a fresh copy (`ArraySlice(varargs, 0)`), `{a, b, ...}` boxes the
prefix and appends the varargs. The element type is forced to `unknown`.

## `table.pack` and `select`

- `table.pack(...)` returns a **fresh** `{unknown}` array. The packed count
  is readable both as `#t` and — matching Lua's packed-table shape — as
  `t.n` (`.n` on any array lowers to `ArrayLen`). Indexing is 0-based like
  all Waluau arrays: `t[0] .. t[t.n - 1]`.
- `select('#', ...)` is the vararg count (`ArrayLen`).
- `select(n, ...)` returns the **single** value at 1-based position `n`;
  negative `n` counts from the end (`len + n`). The index is computed with a
  small CFG branch, and out-of-range positions trap on the array bounds
  check where Lua raises "index out of range". Returning the whole tail
  (`select(n, ...)` as a multi-value) needs runtime-variadic multi-values —
  the same gap as `table.unpack` (waluau-zxju).

## Dynamic operations on `unknown`

The Luau conformance helper `checkresults(e, ...)` needs `#`, indexing, and
`==` on unannotated (gradually-typed, `unknown`) parameters, so these now
lower dynamically:

- `a == b` with an `unknown` side emits a representation dispatch
  (`emit_unknown_eq`): numbers compare numerically across i31/`$boxed_f64`
  (so `1 == 1.0`), booleans by unboxed value, and everything else is
  externalized and decided by a new `js_eq_unknown` host import using
  JavaScript `===` — string content equality, identity for host objects and
  GC values (arrays/records/threads/functions), `nil == nil`. This matches
  Lua primitive equality without metamethods.
- `#v` with `v: unknown` emits `DynLen`: a `ref.test` chain over every
  growable-array wrapper type in the module, reading the wrapper's length
  field; non-arrays trap. (Strings-as-unknown are not handled yet.)
- `v[i]` with `v: unknown` emits `DynIndex`: the same dispatch plus a
  per-type bounds check and an element read boxed back into `unknown`.
  Array types whose elements cannot box (`i64`, `u64`, `f32`) fall through
  to the trap. Writes (`v[i] = x`) are not supported.

Supporting pieces:

- Heterogeneous array literals (`{ true, 1, 2, 42 }`) fall back to
  `{unknown}` with boxed elements when no concrete element type is expected;
  `local xs: {unknown} = ...` boxes explicitly.
- Array indices accept any numeric type (an f64 loop variable indexes via a
  truncating cast); non-integral or out-of-range indices trap.
- `nil` boxes into `unknown` as a null anyref (e.g. `f(nil)` into a vararg
  or unknown parameter).
- Numeric unboxes out of `unknown` (`Cast unknown -> i32/u32/f64`) dispatch
  on the runtime representation, since integer-valued literals typed f64 box
  as `$boxed_f64` while small integers box as i31.

## Conformance coverage

`conformance/varargs_forwarding.walu`, `varargs_table_literal.walu`,
`table_pack.walu`, `select_n.walu`, `unknown_equality.walu`,
`unknown_len_index.walu`, and `varargs_checkresults.walu` (the pcall suite's
`checkresults` helper adapted to 0-based indexing, running end to end).
