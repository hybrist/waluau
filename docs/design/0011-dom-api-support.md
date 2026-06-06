# 0011: DOM API Support — Type System and Expressivity Gaps

## Status

Draft.

## Goal

Identify the language and type-system features that must exist before Waluau
programs can meaningfully consume DOM (and host) APIs. This note does not
implement any of these features; it records what is needed, why, and in what
order, so that each item can be tackled as a focused work item.

The analysis is grounded in the current conformance suite and the existing type
system. Items are ordered roughly by dependency: later items assume earlier ones.

---

## 1. Host function declarations (`declare function`)

### Problem

Every Wasm import that crosses the host boundary today is hard-coded in
`crates/waluau-codegen-wasm/src/host.rs` as a named constant with a fixed
function index (`HOST_IMPORT_COUNT = 17`). There is no source-level mechanism
for user code to introduce a new imported function. All DOM surface would have
to be hand-punched into the compiler's own import table.

### Proposed surface syntax

```lua
declare function getElementById(id: string): Element
declare function addEventListener(target: Element, event: string, handler: () -> unit): unit
declare function setTimeout(cb: () -> unit, ms: i32): i32
```

### Implementation sketch

- **Parser / AST**: recognise `declare function <name>(<params>): <type>` as a
  top-level statement; produce a new `Program::declared_imports` list (separate
  from the existing `functions` list so HIR never sees a body).
- **HIR / type-checker**: register declared functions in the global function
  signature table; validate call sites against the declared signature exactly as
  for regular functions.
- **IR**: add `Instruction::HostCall { import_module: String, import_name:
  String, args: Vec<ValueId>, return_type: Type }`, or repurpose the existing
  `Call` with a distinguished import marker.
- **Codegen**: emit a Wasm import section entry on demand (keyed by
  `(import_module, import_name)`) and route `HostCall` to the correct index.
  The fixed `HOST_IMPORT_COUNT` offset is preserved for built-ins; user imports
  are appended after.

### Acceptance criterion

A conformance test that declares one host function, calls it, and verifies the
result executes correctly in the browser harness.

---

## 2. Opaque / extern host-object types (`type T = extern`)

### Problem

`string` and `bytes` already lower to `externref`, but they are built-in types
with no user-facing mechanism to introduce new host-object types. `Type::Opaque`
exists in the AST/HIR but is only produced internally from named-type resolution;
there is no source syntax to *declare* a new opaque type backed by an
`externref`. Exposing DOM requires names like `Element`, `Event`, `Node`,
`Window`, etc. that:

- are structurally distinct in the type-checker (an `Element` is not assignable
  to a `Node` without an explicit cast);
- lower to `externref` in Wasm;
- can appear in function signatures, record fields, and arrays.

### Proposed surface syntax

```lua
type Element = extern
type Event   = extern
type Node    = extern
```

### Implementation sketch

- **Parser / AST**: recognise `type <Name> = extern` as a special form in a
  `TypeDeclaration`, setting a flag (e.g. `extern_ref: bool`) or producing a
  dedicated `Type::ExternRef(name)` variant.
- **HIR**: resolve such declarations to a nominal type that is distinct from
  every other named type. Nominal distinctness means `Element` is not assignable
  to `Node` even though both lower to `externref`.
- **Codegen**: `wasm_type` already maps `Type::String` / `Type::Bytes` to
  `externref_val_type()`; extend it to do the same for user-declared extern
  types.

### Acceptance criterion

A conformance test that declares an extern type, receives one from a declared
host function, passes it back to another host function, and verifies round-trip
identity without trapping.

---

## 3. Nullable modifier on opaque host types (`T?`)

### Problem

DOM routinely returns nullable references (`getElementById` → `Element | null`,
`parentNode` → `Node | null`). The language currently has no `nil` literal, no
`T?` form, and no null-check narrowing.

As a first slice, nullable is restricted to *extern opaque types* only. A
general `T?` over all types is out of scope here.

### Proposed surface syntax

```lua
type Element = extern

declare function getElementById(id: string): Element?

local el: Element? = getElementById("app")
if el ~= nil then
    -- el is narrowed to Element here
    doSomething(el)
end
```

### Implementation sketch

- **Parser / AST**: parse `T?` as a postfix nullable modifier, producing
  `Type::Nullable(Box<Type>)`. Initially gated so only extern opaque types are
  accepted as the inner type (anything else is a type error at HIR).
- **Lexer**: `nil` keyword (excluded from V0; admitted here for the nullable
  context).
- **HIR / type-checker**:
  - `nil` literal has type `T?` when the expected type is `Type::Nullable(T)`,
    or a bare `Type::Nil` otherwise.
  - `x ~= nil` and `x == nil` comparisons introduce a narrowing obligation: in
    the truthy branch of `x ~= nil`, `x` has type `T`; in the false branch, `T?`.
  - Assigning `T?` to a position expecting `T` is a type error; a narrowed
    branch or an explicit unwrap (e.g. `x::T`) is required.
- **Codegen**: `externref` in Wasm is already nullable (`ref null extern`); the
  null test lowers to `ref.is_null` and the narrowing branch to a conditional.

### Acceptance criterion

A conformance test that receives a nullable extern reference, checks for `nil`,
and branches on the result, with both the null and non-null paths exercised.

---

## 4. User-visible discriminated union types and `match`

### Problem

`Type::TaggedVariant` and `Type::TaggedUnion` already exist in the AST and are
used internally for coroutine resume results. There is no source syntax to
*declare* a union type or *match* on one.

DOM needs this for event sub-types, `Promise` results, `Node.nodeType`-
discriminated access, and generally for safe error handling patterns.

### Proposed surface syntax

```lua
type Result<T> = Ok(T) | Err(string)

function parse(s: string): Result<i32>
    -- ...
end

local r = parse("42")
match r do
    Ok(v) -> assert(v == 42)
    Err(_) -> assert(false)
end
```

### Implementation sketch

- **Parser / AST**: `type Name = Tag(T) | Tag2(U)` declaration form (extends the
  existing `TypeDeclaration`); `match expr do <arms> end` statement.
- **HIR**: resolve union declarations to `Type::TaggedUnion`; type-check `match`
  arms; enforce exhaustiveness (all tags covered or a wildcard present).
- **IR**: add `Instruction::TaggedUnionTag { value: ValueId }` (returns the
  integer tag) and `Instruction::TaggedUnionPayload { value: ValueId, tag:
  String, ty: Type }` (extracts the payload). The coroutine lowering path
  already encodes this pattern internally.
- **Codegen**: Wasm-GC struct-based tagged union layout, analogous to the
  internal coroutine result type.

### Acceptance criterion

A conformance test covering: construction of each variant, single-arm `match`,
multi-arm `match`, exhaustiveness error for missing arms (compile-time), and
wildcard `_`.

---

## 5. Generic type constructors

### Problem

The generics MVP (design 0009) deliberately deferred generic *type declarations*.
Function-level type parameters exist, but `Array<T>`, `Promise<T>`, `Result<T,
E>`, etc. cannot be named. DOM's `Promise<T>`, `ReadableStream<T>`, and any
generic container type require this. Without it, every specialisation
(`Promise<string>`, `Promise<i32>`) must be a separate opaque alias.

### Proposed surface syntax

```lua
type Promise<T>       = extern
type Result<T, E>     = Ok(T) | Err(E)
type Box<T>           = { value: T }
```

Type arguments at use sites:

```lua
declare function fetch(url: string): Promise<string>
local p: Promise<string> = fetch("https://example.com")
```

### Implementation sketch

- **Parser / AST**: `TypeDeclaration` already has `type_params: Vec<String>`; the
  parser just needs to populate it for non-function declarations (currently
  unused).
- **HIR**: generalise `resolve_type_aliases` to substitute type parameters when a
  `Named { name, type_args }` type refers to a parameterised declaration;
  validate arity.
- **Codegen**: each concrete instantiation (`Promise<string>`, `Promise<i32>`)
  produces its own Wasm type entry, mirroring how generic functions already
  monomorphise.

### Acceptance criterion

A conformance test with a generic extern type instantiated at two different type
arguments, verifying they are type-distinct and each can be returned from and
passed to separately-declared host functions.

---

## 6. Method dispatch on host objects

### Problem

The language supports `obj:method(…)` for Waluau-defined table methods, but
those desugar to a wasm-internal struct field lookup + `call_indirect`. There is
no mechanism to say "calling `el:addEventListener(…)` should emit a
host-imported function call". DOM is OOP: `element.addEventListener(…)`,
`node.appendChild(child)`, `promise.then(f)`. Without host-method dispatch,
every DOM method must be wrapped as a free function.

### Proposed surface syntax (option A — method on `declare`)

```lua
declare function Element:addEventListener(event: string, cb: () -> unit): unit
declare function Node:appendChild(child: Node): Node
```

Colon-form `declare` registers the function as a method of the named extern
type; `el:addEventListener(…)` desugars to the host import call.

### Implementation sketch

- Extend `declare function` to accept `Type:method` names.
- During HIR method-call resolution, check whether the receiver type is an
  extern opaque type and whether a matching declared method exists; if so, emit
  `HostCall` instead of `CallValue`.
- Codegen path is identical to free `declare function` calls.

### Acceptance criterion

A conformance test that calls a method on an extern host type using colon syntax
and validates the result.

---

## 7. Variadic arguments

### Problem

Many host APIs accept a variable number of arguments (`console.log(…)`,
`CustomEvent` detail options). The language has no `...args` / varargs form.
This is not a blocker for the first DOM slice but becomes necessary quickly.

### Proposed surface syntax

```lua
declare function console_log(...: string): unit
```

### Implementation sketch

- **Parser / AST**: `...` parameter form (rest parameter, typed).
- **HIR**: type-check call sites; verify that spread arguments match the rest
  parameter type.
- **IR / Codegen**: for host imports this lowers to however the Wasm import is
  declared. User-defined variadic functions would need an array representation,
  but that is out of scope here.

### Acceptance criterion

A conformance test calling a declared variadic host function with zero, one, and
multiple arguments.

---

## Priority Order

| # | Feature | Issue | Depends on | Priority |
|--:|---------|-------|-----------|---------|
| 1 | `declare function` | [waluau-rw8e](../../../bd/issues/waluau-rw8e) | — | High |
| 2 | `type T = extern` | [waluau-oeb5](../../../bd/issues/waluau-oeb5) | — | High |
| 3 | Nullable `T?` on extern types | [waluau-s21a](../../../bd/issues/waluau-s21a) | 2 | High |
| 4 | User-visible discriminated unions + `match` | [waluau-ngdl](../../../bd/issues/waluau-ngdl) | — | Medium |
| 5 | Generic type constructors | [waluau-xzbg](../../../bd/issues/waluau-xzbg) | 2, 4 | Medium |
| 6 | Method dispatch on host objects | [waluau-6mcb](../../../bd/issues/waluau-6mcb) | 1, 2 | Medium |
| 7 | Variadic arguments | [waluau-s2ui](../../../bd/issues/waluau-s2ui) | 1 | Low |

Items 1–3 are the minimum viable set: they let you write a thin DOM shim, handle
nullable returns from `getElementById`-style APIs, and pass typed objects
through function boundaries. Items 4–7 are needed for ergonomic, type-safe DOM
programming at scale but are not required for the first prototype.

---

## Non-Goals

- Full Lua `nil` semantics and general `T?` over non-extern types (deferred
  until the type system has a broader nullable story).
- Structural subtyping between extern types (e.g. `HTMLElement <: Element`);
  nominal casting suffices for the first slice.
- Wasm component-model or interface-types integration; this design targets the
  existing `externref`-based host ABI.
- Automatic TypeScript `.d.ts` → Waluau declaration generation (useful later,
  out of scope here).
