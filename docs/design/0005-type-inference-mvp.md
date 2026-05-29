# 0005: Type Inference MVP

## Status

Accepted.

## Goal

Introduce a minimal, deterministic type inference slice that reduces required annotations while preserving current static guarantees.

This MVP is intentionally narrow: it focuses on local binding inference, expression constraint solving, and non-recursive function return inference.

## Scope

Inference is introduced for:

- local variable declarations with initializer expressions
- non-empty array literal element types
- expression result types in typed contexts
- top-level and local function return types for non-recursive functions

Inference is not introduced for:

- function parameter types (still required)
- recursive function return types
- polymorphism/generics
- union/intersection types
- table shape inference
- implicit `nil`-based flow typing

## Syntax and Annotation Policy

### Required annotations

- all function parameters
- typed empty arrays when no contextual element type is available

### Optional annotations

- local declarations with initializer expressions
- function return types for non-recursive functions

When an optional annotation is present, the inferred type must match the declared type under the same coercion rules as assignment.

## Type Lattice and Widening

MVP inference reuses existing scalar rules:

- exact match is preferred
- non-lossy widening is allowed
- narrowing or lossy conversion requires explicit cast

Current numeric common-type behavior remains the source of truth:

- `i32 + i64 -> i64`
- `i32 + f64 -> f64`
- `i64 + f64` is an error unless explicit cast disambiguates

Arrays are invariant in element type.

## Constraint Model

Inference uses equality and compatibility constraints:

- equality constraints from assignment/return/branch unification
- compatibility constraints from operator and call-site expectations
- contextual constraints from expected type positions

Constraint solving must be deterministic for identical AST input.

If multiple valid solutions remain and there is no deterministic tie-breaker, inference fails with an ambiguity diagnostic.

## Expression Inference Rules

### Literals

- boolean literals infer `bool`
- numeric literals infer a numeric type from context when available
- without context, numeric literals use existing literal defaulting behavior

### Unary and binary operators

- operators contribute operand and result constraints based on existing typing rules
- `not` requires `bool`
- arithmetic requires numeric operands and produces inferred common numeric type when it exists

### Calls

- callee must infer to function type
- argument expressions are constrained by parameter types
- call expression type is function return type

### If-expressions

- condition must infer `bool`
- both branches are inferred under shared expected context
- branch result types must unify to one type, else inference fails

### Array literals

- non-empty array literals infer element type by folding a common element type across elements
- empty array literals require contextual element type or explicit annotation; otherwise inference fails

## Statement Inference Rules

### Local declarations

For `local x = expr`:

- infer `expr` type as `T`
- bind `x : T`

For `local x: A = expr`:

- infer `expr` under expected type `A`
- require `expr` type compatible with `A`

Reassignment (`x = expr`, `x += expr`) never changes `x`'s bound type.

### Function return inference (non-recursive)

For unannotated non-recursive function `f`:

- collect inferred type of each reachable `return expr`
- unify return expression types to a single return type `R`
- assign function type `(...params) -> R`

Failures:

- no return in a required-return context
- incompatible return branch types
- recursive self-reference (explicitly unsupported in MVP)

Annotated returns continue to be checked against inferred return expressions.

## Soundness Boundaries

MVP remains sound within these explicit boundaries:

- no implicit narrowing
- no inference across recursive cycles
- no introduction of `any`/dynamic escape type
- inference failure is reported rather than guessed

When the engine cannot prove a unique type, it must fail closed with diagnostics.

## Diagnostics Contract

Inference failures should map to stable categories:

- `inference/ambiguous`
- `inference/conflict`
- `inference/unsupported`
- `inference/missing-context`

Each diagnostic should include:

- source span (where available)
- short root cause
- actionable next step (add annotation, add cast, simplify expression, etc.)

## Non-Goals and Future Work

Deferred beyond MVP:

- recursive return inference via fixed-point solving
- parameter type inference
- generic inference
- richer flow-sensitive refinement
- cross-function whole-program inference

## Acceptance Mapping

This doc satisfies the spec issue by defining:

- typing/inference rules for MVP constructs
- fallback behavior and deterministic failure paths
- explicit success/failure boundaries
- non-goals for recursive and advanced constructs
