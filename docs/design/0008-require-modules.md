# 0008: Relative Imports via `require`

## Status

In progress (single-function exports implemented; table namespace exports added).

## Goal

Introduce a Luau-style `require` so a program can be split across multiple
`.walu` files, with imports resolved relative to the importing file. This is the
first slice of the M6 milestone from [0001](0001-overview.md).

The design must:

- keep `require` paths relative and resolved against the requiring file
- give each file a single exported value, like a Luau module
- detect circular imports with a clear diagnostic
- avoid threading module-awareness through the HIR, IR, and codegen stages

## Source Semantics

### Importing

A module is imported with `require`, whose argument is a string literal path:

```lua
local add = require("./add")
```

- Paths must be relative and begin with `./` or `../`.
- The `.walu` extension is appended when the path has no extension.
- Resolution is relative to the directory of the file containing the `require`.

String literals exist **only** as the argument to `require`. The language still
has no string value type (that remains future work), so a string literal in any
other position is a compile error.

### Exporting

A module exports a single value through a trailing top-level `return`:

```lua
function add(a: i32, b: i32): i32
    return a + b
end

return add
```

For this first slice the exported value is either the name of a top-level
function or a table of functions:

```lua
return {
    add = function (a: f64, b: f64): f64
        return a + b
    end,
}
```

Table fields may be inline `function ... end` expressions, names of top-level
functions in the same module, or bindings from top-level `require` imports
(including namespace members like `ns.add`). The `return` must be the last item
in the file. The entry module (the file passed to the compiler) may also have
one, but it is ignored.

Imported modules may bind `require` results at the top level and re-export them:

```lua
local double: (i32) -> i32 = require("./single-export")
local ns = require("./multi-export")

return { double = double, add = ns.add }
```

### Using an Import

A single-function export works as before:

```lua
local add: (i32, i32) -> i32 = require("./add")
local total: i32 = add(2, 3)
```

A table export binds a local namespace; members are called with dot syntax:

```lua
local m = require("./ops")
local total: f64 = m.add(2.0, 3.0)
```

This relies on a top-level function name being usable as a first-class value.
That capability is added as part of this work: a bare top-level function name
lowers to a capture-free function reference (a `funcref` placed in the module's
function table), which the existing indirect-call machinery already supports.

## Implementation

Module resolution and linking live entirely in `waluau-driver`; the lexer,
parser, and AST gain only the surface syntax. The later stages (`waluau-hir`,
`waluau-ir`, `waluau-codegen-wasm`) never observe a module boundary.

### Front end

- **Lexer**: adds a `Str` token for double-quoted literals, supporting the
  `\"`, `\\`, `\n`, and `\t` escapes.
- **AST**: adds `Expr::Require(String)` and an `export: Option<Expr>` field on
  `Program`.
- **Parser**: recognises `require("...")` as a dedicated node and folds a
  trailing top-level `return` into `Program::export`.

### Linker

`waluau-driver` resolves the graph and merges it into one `Program`:

1. **Load**: starting from the entry file, parse each module and recursively
   load every path it requires. Modules are de-duplicated by canonical path
   (so diamonds load once), and a path currently on the resolution stack is a
   circular import error.
2. **Mangle**: every non-entry module's top-level functions are renamed with a
   unique per-module prefix, so identically named functions from different files
   cannot collide. The entry module keeps its original names, preserving its Wasm
   export names.
3. **Rewrite**: each `require(...)` node is replaced with a reference to the
   imported module's (mangled) exported function. References to a module's own
   functions are rewritten with lexical-scope awareness, so a local variable that
   shadows a function name is left untouched.

The merged `Program` then flows through the existing pipeline unchanged.

## Constraints and Limitations

These keep the first slice small; each can be lifted later.

- Imported modules now contribute their top-level statements to a single
  synthesized init sequence. Dependencies initialize before the modules that
  require them, and the entry module initializes last.
- Table exports are limited to fixed named fields mapping to functions (no
  arbitrary table types or non-function values). General table support remains M4.
- Wasm exports are limited to the entry-facing function surface; mangled module
  internals and synthesized helper functions stay internal.

## Alternatives Considered

- **Multi-file awareness throughout the pipeline** (file IDs in spans, per-module
  symbol tables, cross-module linking in the IR). More faithful, but a large
  change for little immediate benefit. Driver-level merging gives working
  relative imports today and leaves that evolution open.
- **Error on duplicate names instead of mangling.** Simpler, but it leaks one
  module's naming choices into another and breaks encapsulation. Mangling keeps
  modules independent.
