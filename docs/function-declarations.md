# Function declarations and module interfaces

Waluau has four function declaration forms. They differ in binding scope,
declaration order, rebinding, and authored exposure; none creates an ambient
global.

| Form | Binding | Order | Rebinding | Module interface |
| --- | --- | --- | --- | --- |
| `function f()` | private module function | hoisted | immutable | private unless selected by a legacy trailing return |
| `export function f()` | exported module function | hoisted | immutable | named member of the module |
| `local function f()` | lexical closure | declaration order | rebindable | private |
| `const function f()` | lexical closure | declaration order | immutable | private |

Use a plain module function for implementation shared across functions in one
file:

```waluau
function clamp_score(score: i32): i32
    if score < 0 then
        return 0
    elseif score > 100 then
        return 100
    end
    return score
end
```

Use a lexical declaration when placement within a block matters. Choose
`local function` when later assignment is intentional, and `const function`
when it is not:

```waluau
local function next_score(score: i32): i32
    return score + 1
end

const function score_label(score: i32): string
    return tostring(score)
end
```

Use `export function` for a named public value:

```waluau
export function score_label(score: i32): string
    return tostring(score)
end
```

An exported function name must be simple. Qualified declarations such as
`export function State.new()` and nested exported declarations are invalid.
Export ordinary module functions only at the module's top level.

## `require` interfaces

Explicit function exports form a namespace:

```waluau
-- scores.walu
function clamp_score(score: i32): i32
    if score < 0 then
        return 0
    elseif score > 100 then
        return 100
    end
    return score
end

export function label(score: i32): string
    return tostring(clamp_score(score))
end
```

```waluau
local scores = require("./scores")
print(scores.label(120))
```

`scores.label` is public; `scores.clamp_score` does not exist. Hover,
go-to-definition, references, completion, and imported-member lookup use this
same interface.

Trailing returns remain supported for modules whose value does not fit named
declarations. A returned function is imported as a callable value:

```waluau
-- increment.walu
function increment(value: i32): i32
    return value + 1
end
return increment
```

```waluau
local increment = require("./increment")
print(tostring(increment(41)))
```

A returned table is a legacy namespace. Only its selected fields are public,
and a field may rename a private declaration:

```waluau
-- math_helpers.walu
function internal_double(value: i32): i32
    return value * 2
end
function private_helper(): i32
    return 0
end
return { double = internal_double }
```

```waluau
local helpers = require("./math_helpers")
print(tostring(helpers.double(21)))
```

`helpers.private_helper` and `helpers.internal_double` are not members. A
module cannot combine an `export function` declaration with a trailing return;
choose one value-interface style. Exported type and enum declarations may
coexist with either style.

## Browser Wasm exports

Waluau targets Wasm GC in browsers. In the entry file, each authored
`export function` is a stable browser-visible Wasm export in development and
production builds. Explicit exports from required dependency files remain
module-linking metadata and are not leaked as Wasm exports.

A trailing return defines what another Waluau module receives through
`require`; it does not define the entry file's browser Wasm exports. The
compiler also owns runtime exports needed to initialize and call the module.
Do not treat those runtime names as authored language declarations.

The default CLI output exposes authored entry functions plus required runtime
exports. `--tooling-exports` additionally exposes private entry functions and
marshalling helpers for the playground, tests, and other browser tooling. This
extra surface is instrumentation, not the module interface, and code must not
depend on it in production. `--minimal-exports` is retained as a compatibility
alias for the authored-only default; remove it from new commands.

## Migration

Code that relied on every top-level function appearing in `require` completion
or in a development Wasm export list was relying on tooling exposure.

- Add `export` when a simple named function is part of a module's public
  namespace or the entry file's browser interface.
- Keep plain `function` for private, hoisted module implementation.
- Preserve a trailing return for callable modules, renamed fields, constant
  fields, re-exports, or inline legacy functions that do not fit declaration
  exports.
- Replace `module.member` use of a callable legacy module with a direct call to
  the required value, or return a table if a namespace is intended.
- Remove `--minimal-exports`; authored-only output is already the default.
- Use `--tooling-exports` only where browser tooling deliberately needs the
  additional callable surface.

The compiler reports qualified exported functions, mixed value interfaces,
and attempts to rebind immutable module functions directly. Editors surface
those compiler diagnostics rather than adding a second, potentially duplicate
lint. Unused-function diagnostics apply to private simple module functions and
lexical function declarations; explicit exports are public interface members
and are not reported as unused.
