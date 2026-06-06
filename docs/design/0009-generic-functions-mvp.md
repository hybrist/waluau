# 0009: Generic Functions MVP

## Status

Accepted.

## Goal

Define the first generic function slice with explicit type parameters and
deterministic specialization. The MVP should be useful for container helpers and
numeric helper functions without introducing generic inference, trait-style
constraints, or polymorphic first-class function values.

For user/contributor-facing usage and limitation notes, see
[`docs/generic-functions-mvp.md`](../generic-functions-mvp.md).

## Scope

Included:

- generic top-level function declarations
- generic function expressions, including expressions assigned to locals
- generic method declarations (`function t:m<T>(...) ... end`)
- explicit type arguments at generic call sites (`f<T>(...)`)
- type parameters in parameter types, return types, local annotations, array
  element types, and nested function types
- deterministic substitution before normal type checking and lowering

Excluded:

- inferred type arguments
- constraints or bounds on type parameters
- default type parameters
- type parameters on types, aliases, arrays, tables, or modules
- source syntax for polymorphic function value types
- partial application or value-level references to an uninstantiated generic
  function
- explicit type arguments at method call sites (`obj:method<T>(...)`)  — the
  `MethodCall` AST node does not carry `type_args`; generic methods can be
  declared but only called through a desugared field-access form for now

## Source Syntax

Generic type parameters are declared in angle brackets immediately after the
function name for named functions:

```lua
function id<T>(value: T): T
  return value
end
```

Function expressions place type parameters immediately after `function` or after
the optional local self-name:

```lua
local id = function<T>(value: T): T
  return value
end

local loop = function self<T>(value: T): T
  return self<T>(value)
end
```

Calls to a generic function must provide explicit type arguments between the
callee expression and the argument list:

```lua
local x = id<i32>(1)
local y = id<bool>(true)
```

The parser recognizes a type argument list only in call position when `<...>` is
followed by `(`. Other uses of `<` continue to parse as comparison expressions.

### Grammar Extension

This is an additive sketch over the existing grammar:

```text
TypeParamList ::= "<" Identifier ("," Identifier)* ">"
TypeArgList   ::= "<" Type ("," Type)* ">"

FunctionDecl  ::= "function" Identifier TypeParamList? "(" ParamList? ")"
                  ReturnType? Block "end"

MethodDecl    ::= "function" Identifier ":" Identifier TypeParamList?
                  "(" ParamList? ")" ReturnType? Block "end"

FunctionExpr  ::= "function" Identifier? TypeParamList? "(" ParamList? ")"
                  ReturnType? Block "end"

CallExpr      ::= Callee TypeArgList? "(" ArgList? ")"
```

`MethodDecl` is desugared during HIR lowering into a regular generic function
stored on the table. The colon-call form `obj:method<T>(...)` does not yet
accept a `TypeArgList`; that extension is tracked separately.

Type parameter names share the identifier token shape with value names, but they
live in a separate type namespace.

## Type Parameter Scoping

Each generic function introduces a fresh type-parameter scope.

- A type parameter is visible throughout that function's signature and body.
- Type parameters are not visible before the `function` declaration, after its
  `end`, or in sibling functions.
- Nested generic functions may declare their own type parameters. A duplicate
  name in the same type-parameter list is an error.
- Shadowing an outer type parameter in a nested generic function is rejected in
  the MVP to keep diagnostics and substitution simple.
- Value names and type parameter names do not collide. `function f<T>(T: i32):
  T` is valid at name-resolution time only if the return `T` is read from the
  type namespace and the parameter name `T` is read from the value namespace.

Example:

```lua
function outer<T>(value: T): T
  local inner = function<U>(other: U): U
    return other
  end
  return value
end
```

`T` is available in `outer`; `U` is available only in `inner`.

## Typing Model

A generic function declaration has a type scheme:

```text
forall <T1, ..., Tn>. (P1, ..., Pm) -> R
```

The scheme is an internal compiler type, not a source-level type. Source function
types remain monomorphic:

```lua
local f: (i32) -> i32 = function(x: i32): i32
  return x
end
```

There is no syntax for `forall<T>. (T) -> T` in the MVP, and a generic function
cannot be assigned, passed, or returned while still uninstantiated.

### Type Arguments

For `callee<A1, ..., An>(args...)`:

- the callee must resolve to a generic function scheme
- the number of type arguments must exactly match the number of type parameters
- each type argument must be a valid type in the current type scope
- each type argument must be concrete after substituting any enclosing type
  parameters

Inside a generic function, an in-scope type parameter may be forwarded as a type
argument:

```lua
function first<T>(items: {T}): T
  return get<T>(items, 0)
end
```

After substituting `T`, `get<T>` specializes with the same concrete type as the
enclosing `first` specialization.

### Substitution

Before checking a generic call body, the compiler substitutes type parameters
with type arguments throughout the function signature and body:

```lua
function pair_first<T>(left: T, right: T): T
  return left
end

local n = pair_first<i32>(1, 2)
```

The `i32` specialization is checked as if the declaration were:

```lua
function pair_first_i32(left: i32, right: i32): i32
  return left
end
```

Substitution is structural:

- `T` becomes the selected type argument
- `{T}` becomes an array of the selected type
- `(T) -> T` becomes a monomorphic function type using the selected type
- unrelated concrete types are unchanged

Normal assignment, call, return, operator, cast, and numeric widening rules run
after substitution. A generic body must type-check for every specialization that
is requested by the program.

## Call Checking

Generic call checking is explicit and closed:

- Calling a generic function without type arguments is an error.
- Supplying type arguments to a non-generic function is an error.
- The specialized parameter types are used as expected types for arguments.
- The specialized return type is the call expression type.
- Type argument inference from ordinary arguments is deferred.

Example:

```lua
function wrap<T>(value: T): {T}
  return { value }
end

local numbers = wrap<i32>(1)
local flags = wrap<bool>(true)
```

`numbers` has type `{i32}` and `flags` has type `{bool}`.

## Valid Programs

```lua
function choose<T>(condition: bool, a: T, b: T): T
  if condition then
    return a
  else
    return b
  end
end

function main(): i32
  return choose<i32>(true, 1, 2)
end
```

```lua
function map_one<T, U>(value: T, f: (T) -> U): U
  return f(value)
end

function inc(x: i32): i32
  return x + 1
end

function main(): i32
  return map_one<i32, i32>(41, inc)
end
```

## Invalid Programs

Missing explicit type arguments:

```lua
function id<T>(value: T): T
  return value
end

local x = id(1)
```

The call to `id` is rejected because generic inference is not part of the MVP.

Wrong type argument arity:

```lua
function same<T>(a: T, b: T): T
  return a
end

local x = same<i32, bool>(1, 2)
```

The call is rejected because `same` declares one type parameter.

Unknown type parameter:

```lua
function bad(value: T): T
  return value
end
```

`T` is rejected because it is not declared in an enclosing type-parameter scope.

Mixed substituted argument types:

```lua
function same<T>(a: T, b: T): T
  return a
end

local x = same<i32>(1, true)
```

The second argument is rejected because the specialized second parameter type is
`i32`.

Uninstantiated generic value:

```lua
function id<T>(value: T): T
  return value
end

local f = id
```

The binding is rejected because the MVP has no source type for polymorphic
function values.

## Failure Modes and Diagnostics

The compiler should fail closed with diagnostics for:

- duplicate type parameter names in one declaration
- nested generic type parameter shadowing
- use of an unknown type name
- type arguments on non-generic callees
- missing type arguments on generic callees
- mismatched type argument count
- uninstantiated generic functions used as values
- generic body specialization failures after substitution
- parser ambiguity where `<...>` is not followed by a call argument list

Diagnostics should point at the smallest useful span: the duplicate type
parameter, the unknown type reference, the type argument list, or the callee.

## Lowering and Specialization

Each distinct `(generic function symbol, concrete type argument list)` pair
produces one monomorphic specialization. Repeated calls to the same pair reuse
the same specialization.

Specialization happens before HIR is lowered into CFG IR. CFG IR, SSA, and Wasm
codegen continue to see only monomorphic function signatures and direct calls.
Generated specialization names are compiler-internal and must be deterministic
for identical input programs.

Recursive generic functions are allowed only when recursive calls provide an
explicit type argument list. The MVP supports recursion at the same type
arguments as the current specialization. Cross-specialization recursion is
deferred because it requires cycle detection over the specialization graph.

## Deferred Decisions

Deferred beyond the MVP:

- inferred type arguments at call sites
- constraints, bounds, or trait-like operator capabilities
- generic type aliases and generic data types
- polymorphic function value types
- explicit type arguments at method call sites — `obj:method<T>(...)` syntax;
  generic method declarations are implemented and desugared, but the
  `MethodCall` AST node does not carry `type_args` yet
- specialization sharing across representation-compatible types
- cross-module generic export/import semantics
- cross-specialization recursive cycles

## Acceptance Mapping

This note specifies:

- declaration syntax for named functions and function expressions
- call-site type argument syntax
- type parameter scoping and namespace behavior
- no-constraint MVP typing semantics
- structural substitution and monomorphic specialization
- valid and invalid examples
- concrete failure modes and deferred feature decisions
