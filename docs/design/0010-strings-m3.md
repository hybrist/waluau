# 0010: String Type (M3)

## Status

Implemented (documentation of existing implementation).

## Goal

Define the string type semantics, representation, and operations for waluau's text processing capabilities. This documents the design decisions that have been implemented and the boundaries with binary data (covered separately in the bytes type).

## String Representation

### Host-Managed External References

Strings use **opaque externref** values via the `wasm:js-string` proposal. This approach:

- Delegates string storage and encoding to the host environment
- Avoids embedding UTF-8/UTF-16 encoding logic in the compiler
- Provides natural interop with JavaScript strings in browser environments
- Eliminates linear memory concerns for text data

**Trade-offs:**
- Host dependency: cannot run in pure Wasm environments without string host imports
- No byte-level access: strings cannot be treated as byte arrays in scalar Wasm
- Performance: string operations require host calls rather than inline Wasm instructions

### String Constants

String literals are collected during compilation and stored in:

1. **Custom Wasm Section** (`waluau.strc`): Binary encoding of all string constants used in the module
2. **Host Import** (`importedStringConstants`): Runtime retrieval of string externref values by index

This approach enables:
- Efficient constant deduplication at compile time
- Host-appropriate string instantiation (UTF-16 in JS, UTF-8 in other hosts)
- Separate string constant loading from module instantiation

## Mutability

Strings are **immutable values**. All string operations return new string values rather than modifying existing ones.

**Rationale:**
- Simpler semantics: no aliasing concerns, predictable behavior
- Consistent with Lua 5.4 string model
- Natural fit for `wasm:js-string` which provides immutable operations
- Avoids complex copy-on-write or reference-counting schemes

**Implications:**
- String concatenation creates new strings
- String operations are purely functional
- No in-place string mutation APIs

## Equality Semantics

String equality uses **deep value comparison** via the `wasm:js-string` `equals` host import.

**Behavior:**
- `"hello" == "hello"` → `true` (same content)
- `"hello" == "world"` → `false` (different content)
- Equality is **not** reference-identity based

**Host Delegation:**
- Encoding normalization (if any) is handled by the host
- Unicode comparison rules follow host string semantics
- Consistent with JavaScript `===` for string values

## Comparison Semantics

String comparison for ordering operations (`<`, `>`, `<=`, `>=`) uses **lexicographic ordering** via the `wasm:js-string` `compare` host import.

**Behavior:**
- `compare(a, b)` returns `-1` (a < b), `0` (a == b), or `1` (a > b)
- Ordering follows host-defined string collation
- Consistent with JavaScript string comparison

**Implementation:**
- `a < b` → `compare(a, b) < 0`
- `a > b` → `compare(a, b) > 0`
- `<=` and `>=` are desugared in HIR, not separate Wasm operations

## Length Semantics

String length via the `#` operator returns the **logical length** as defined by the host environment.

**Host Delegation:**
- JavaScript hosts: code unit count (UTF-16)
- Other hosts: may use code point count or byte count as appropriate
- No guarantee of byte-level length consistency across hosts

**Implementation:**
- `#s` → `wasm:js-string` `length` host import
- Returns integer value
- Consistent with host string `.length` property semantics

## Operations

### Implemented Operations

| Operation | Syntax | HIR Type Rule | Codegen |
|-----------|--------|---------------|---------|
| **Literal** | `"text"` | `→ string` | String constant index |
| **Concatenation** | `s1 .. s2` | `string × string → string` | `wasm:js-string` `concat` |
| **Equality** | `s1 == s2` | `string × string → bool` | `wasm:js-string` `equals` |
| **Comparison** | `s1 < s2`, `s1 > s2` | `string × string → bool` | `wasm:js-string` `compare` |
| **Length** | `#s` | `string → i32` | `wasm:js-string` `length` |
| **Conversion** | `tostring(x)` | `T → string` | Type-specific host imports |
| **Output** | `print(s)` | `string → void` | Host print import |

### Type Checking Rules

1. **String literals** infer to `Type::String`
2. **Concatenation** (`..`) requires both operands to be `string` type, result is `string`
3. **Equality** (`==`) requires both operands to be `string` type when used with strings
4. **Comparison** (`<`, `>`) requires both operands to be `string` type, result is `bool`
5. **Length** (`#`) on string values returns `i32`
6. **Conversion** (`tostring`) accepts numeric, boolean, or string inputs, returns `string`

## Boundaries with Binary Data

### String vs. Bytes Separation

Waluau maintains a **strict separation** between text strings and binary data:

- **Strings** (`string` type): text data with encoding-aware semantics
- **Bytes** (`bytes` type): binary data with byte-indexable storage

**No Byte-Level String Access:**
- Strings do not expose byte indexing (`s[i]` is invalid)
- String length is not necessarily byte count
- No automatic string ↔ bytes coercion

**Explicit Conversion Boundary:**
- `encode(s: string): bytes` - convert text to binary (future)
- `decode(b: bytes): string` - convert binary to text (future)
- Encoding format and error handling defined separately

This separation prevents:
- Accidental encoding bugs from treating strings as byte arrays
- Host-dependent byte representation exposure
- Conflation of text and binary operation semantics

## Host Integration

### Required Host Imports

String support requires the following host imports from `wasm:js-string`:

| Import | Signature | Purpose |
|--------|-----------|---------|
| `equals` | `(externref, externref) → i32` | String equality |
| `concat` | `(externref, externref) → externref` | String concatenation |
| `compare` | `(externref, externref) → i32` | Lexicographic ordering |
| `length` | `(externref) → i32` | String length |

Additional conversion and I/O imports:
- `js_tostring_*` variants for numeric/boolean conversion
- `print` for string output
- `importedStringConstants` for constant loading

### JavaScript Host Behavior

In browser/Node.js environments:

1. **String constants** are instantiated as JavaScript string values
2. **String operations** delegate to native JavaScript string methods
3. **Encoding** follows JavaScript UTF-16 string semantics
4. **Interop** provides seamless string passing between Wasm and JS

## Future Extensions

### Potential Additions (Out of Current Scope)

- **String slicing**: `s[i:j]` for substring extraction
- **String interpolation**: `"hello {name}"` syntax
- **Pattern matching**: regex or glob-style string operations
- **Case conversion**: `tolower()`, `toupper()` functions
- **String formatting**: printf-style or templating operations

### Bytes Integration

Future explicit conversion APIs between strings and bytes:
- Encoding specification (UTF-8, UTF-16, etc.)
- Error handling for invalid sequences
- Round-trip guarantees and normalization behavior

### Performance Optimizations

- **Rope trees**: efficient concatenation of many strings
- **Interning**: deduplication of identical string values
- **Streaming**: large string processing without full materialization

## Implementation Status

### Completed
- ✅ AST representation (`Type::String`, `Expr::String`)
- ✅ HIR type checking for all string operations
- ✅ IR instructions (`String`, `ToString`, `Print`)
- ✅ Wasm codegen for constants, equality, concatenation, conversion
- ✅ Host imports and custom section handling
- ✅ String constant collection and deduplication
- ✅ Conformance tests (`conformance/strings.walu`)

### In Progress
- ⚠️ String comparison (`<`, `>`) codegen implementation
- ⚠️ String length (`#`) codegen implementation  
- ⚠️ Driver E2E integration tests

### Future
- ⏳ Explicit string/bytes conversion APIs
- ⏳ Advanced string operations (slicing, formatting, etc.)
- ⏳ Performance optimizations (ropes, interning)

## Conclusion

The waluau string type provides a clean text processing abstraction built on host-managed string values. By delegating encoding and storage to the host environment, waluau avoids embedding complex Unicode handling while providing natural interop with existing string APIs.

The strict separation between strings and bytes prevents common encoding pitfalls and maintains clear semantic boundaries between text and binary data domains.