# Development DWARF contract for Chrome

Status: compatibility contract established by `waluau-fdhp.2`; development
emission is available behind an explicit compiler option.

Waluau development builds use conventional external DWARF for source-level
debugging of browser Wasm GC. Chrome's language-extension API associates a
Wasm module with an extension that understands `ExternalDWARF`; the API is not
restricted to a source language. Google's supported extension is branded for
C/C++, so line mapping works for Waluau while rich Waluau value semantics do
not come for free.

The reproducible compatibility and compiler-output probes are in
[`fixtures/dwarf-chrome-wasm-gc`](../fixtures/dwarf-chrome-wasm-gc/README.md).

## Enabling development mappings

The CLI flag is `--development-dwarf`:

```sh
cargo run -p waluau-cli -- path/to/main.walu \
  -o path/to/main.wasm --emit-js --development-dwarf
```

The command writes `main.wasm` and `main.debug.wasm`. Library callers use
`waluau_driver::CompileOptions { development_dwarf: true }` with
`compile_source_artifacts_with_options`, `compile_file_artifacts_with_options`,
or `CompilerSession::build_root_with_options`; the returned artifacts contain
the runtime Wasm and optional companion separately. Bytes-only helpers reject
the option because one byte vector cannot represent both required files.
Codegen-only callers receive the same two fields in `EmitResult`.

The Vite plugin passes the option automatically for dev-server and test
compiles. It serves the sibling from `.waluau/` through Vite's normal file
serving. Production builds do not pass the option. Loading the game fetches
only the runtime Wasm; Chrome resolves and fetches the companion when DevTools
attaches. The generated JavaScript uses `WebAssembly.compileStreaming` for its
URL-based path so Chrome retains the module's HTTP URL and can resolve that
relative sibling; callers supplying bytes directly must preserve an equivalent
resolvable debug-symbol URL themselves.

The runtime Wasm contains `external_debug_info` naming its relative sibling and
never contains `.debug_*`. The standalone companion is a valid Wasm container
with `.debug_abbrev`, `.debug_info`, and `.debug_line`; inline strings make
`.debug_str` unnecessary. For compatibility with Chrome's official extension,
the companion is a debugger-only snapshot of the matching runtime module plus
those sections; its duplicated code is never fetched by the page. With the
option omitted, the reference and companion are absent and the output follows
the unchanged default encoding path.

This gate is independent of the Rust profile used to build the compiler:
neither a debug nor a release `waluau` executable emits DWARF unless the caller
sets `development_dwarf` or passes `--development-dwarf`. Compiler tests compare
the pre-option API with explicit default options byte for byte, and the browser
verifier builds the same two-file program in both modes before launching
Chrome. The verifier reports the tiny runtime reference overhead separately
from the companion size. On 2026-08-22 the two-file fixture was 876 bytes by
default, 998 bytes for the development runtime (+122 bytes / 13.9%), and 1,450
bytes for the debugger-only companion. The companion was not requested by the
ordinary page. Run the verifier to measure current output rather than treating
one fixture as an artifact-size budget.

The compiler derives browser-resolvable, slash-separated paths relative to the
common authored source directory and records `.` as `DW_AT_comp_dir`. It never
embeds the absolute build directory. Line rows use final instruction offsets
relative to the Code section contents and only include IR operations marked
`SourceOrigin::Authored`; synthetic helpers have Wasm names but no line rows or
subprogram DIEs.

## Minimum emission contract

Use DWARF 4 for the first implementation. DWARF 4 and 5 both loaded in the
official extension, but version 4 needs fewer section and form variants.

Emit `external_debug_info` in the runtime module with its payload holding the
companion's relative URL as a length-prefixed WebAssembly UTF-8 string. Emit
each DWARF payload in the standalone Wasm container as a custom section with
its conventional name. The line-debugging baseline is:

- `.debug_abbrev` and `.debug_info`, with a compile-unit DIE that references
  the line program and subprogram DIEs for authored functions;
- `.debug_line`, with rows at WebAssembly instruction boundaries for every
  breakable authored statement; and
- `.debug_str` only when DIEs use string-table forms. Inline `DW_FORM_string`
  can avoid it. Likewise, `.debug_ranges` is unnecessary for contiguous
  functions represented by `DW_AT_low_pc` and `DW_AT_high_pc`.

Removing any of `.debug_info`, `.debug_abbrev`, or `.debug_line` made the
official extension discover no authored sources. Removing `.debug_ranges`
preserved discovery, bidirectional mapping, and function names in the probe.
Removing `.debug_str` preserved line mapping but erased function names because
the carrier's DIEs used string-table references.

Keep the existing Wasm `name` section. It remains the source of useful names in
raw stacks and in unmapped generated code.

### Addresses and line rows

Every DWARF code address is the byte offset of the instruction's first byte
relative to the start of the **Code section contents**. It is not a function
index, function-relative offset, linear-memory address, or absolute file
offset. Chrome adds the Code section's module offset when setting its raw Wasm
breakpoint. Rewriting function bodies after DWARF is generated invalidates all
following mappings, even if the Wasm remains valid.

Emit a useful, nonzero source column for each statement row. In the probe,
Clang's normal column rows let a gutter breakpoint with an unspecified column
normalize to the first mapped expression and bind. A `-gno-column-info`
variant still appeared in `getMappedLines`, but reverse source-to-raw queries
returned no ranges.

### Compile-unit language

Waluau has no assigned DWARF language code. Use `DW_LANG_lo_user` (`0x8000`)
for the compile unit until a code is assigned, and identify Waluau in
`DW_AT_producer`. Do not claim C or C++ merely to match the extension's name.
Changing the probe from `DW_LANG_C11` to `DW_LANG_lo_user` preserved source
discovery and bidirectional line mapping in extension 0.2.5854.1. The extension
did not return a DWARF-derived function frame for the user-range language code,
so the Wasm `name` section remains necessary for useful raw frame names.

This policy intentionally promises mapping only. C/C++ expression parsing and
type formatting are not Waluau semantics.

### Source paths

Store URL-resolvable, slash-separated relative paths and use `.` as the compile
directory. The probe's `dwarf_chrome_probe.walu` path resolved relative to the
Wasm URL and Chrome fetched the authored file over HTTP. The extension worker
reports that HTTP URL, while the full DevTools Sources workspace can present it
as a `wasm://wasm/<relative-path>` source. Absolute build-machine paths require
a user-configured path substitution in the extension and should not be the
default. The development server must serve every referenced source.

### Authored and synthetic functions

Give authored functions subprogram DIEs and line rows. Keep compiler-generated
helpers out of the authored line table and give them descriptive Wasm names.
The probe's GC helper executes successfully, but the extension reports no
source location or DWARF function frame for its offset. DevTools therefore
falls back to raw Wasm if execution steps into it, which is preferable to
attributing generated instructions to a misleading authored line.

## Observed Chrome behavior

Tests on 2026-08-22 used stable Chrome 151.0.7922.173 and the official C/C++
DevTools Support extension 0.2.5854.1. The exact extension worker was exercised
inside stable Chrome. Full DevTools UI automation used Chrome for Testing
148.0.7778.96 because the extension was not installed in the personal stable
profile and branded Chrome ignored the temporary `--load-extension` request.
Repeat the README's short manual procedure in stable Chrome after installing
the extension when validating the production emitter.

| Surface | Result | Observation |
| --- | --- | --- |
| Wasm GC validation and execution | Pass in stable Chrome 151 | The module validated; `struct.new`/`struct.get` returned 42. |
| External DWARF discovery | Pass in Chrome for Testing 148 and stable extension worker | V8 classified the runtime as `ExternalDWARF`; the official worker fetched the sibling and discovered both authored sources. |
| Ordinary page network loading | Pass | The runtime page requested HTML, JavaScript, and runtime Wasm only; it did not request `.debug.wasm`. |
| Authored source discovery | Pass in stable extension worker; pass in full Chrome for Testing UI | The worker returned the HTTP `.walu` URL; the Sources workspace registered the authored relative path. |
| Line breakpoint binding | Pass in full UI | A line-4 gutter-equivalent breakpoint normalized to column 14 and bound to the Wasm module. |
| Source stepping | Pass in full UI | Step-over moved from Waluau line 4 to line 5. |
| Paused call-frame source mapping | Pass in full UI | `inner` mapped to line 4 and its caller `run` mapped to line 13; both pointed at the authored `.walu` URL. |
| `console.error(error)` | Raw stack in stable Chrome 151 | Frames used name-section names plus `wasm-function[N]:0x…`; DWARF did not rewrite the rendered stack. |
| `Error.stack` | Raw stack in stable Chrome 151 | Same Wasm names and byte offsets as the Console error. |
| Uncaught-exception presentation | Raw stack in stable Chrome 151 | The Console/page-error text stayed raw. If DevTools pauses on the exception, the paused frames can still be source-mapped separately. |
| Primitive C-carrier locals | Not a Waluau result | The carrier can expose C values, but that is not evidence of Waluau value semantics. |
| Wasm GC locals and aggregates | Blocked by the extension bridge | The extension serializes numeric/vector Wasm values, rejects reference values, and exposes aggregate objects through linear-memory addresses rather than GC-object traversal. No inspection pass is claimed. |
| Synthetic GC helper | Deliberately unmapped | It remains executable and named at the Wasm layer; source and DWARF-function queries return empty results. |

The stack surfaces are intentionally separate: DWARF improves the debugger's
paused Call Stack, breakpoints, and stepping, but it does not mutate JavaScript
`Error.stack` strings or Console stack serialization. `console.error`,
`Error.stack`, and uncaught Console-rendered Wasm stack strings therefore keep
name-section function names plus Wasm offsets when Chrome supplies a stack
string. Chrome for Testing 148 returned `null`/empty stacks for the compiler's
tagged `WebAssembly.Exception`; that absence is also not repaired by DWARF.

### Production-emitter browser verification

The opt-in verifier documented in the fixture README was run on 2026-08-22 with
Chrome for Testing 148.0.7778.96 and official extension 0.2.5854.1 against
fresh `waluau --development-dwarf` output. It automated source discovery and
both mapping directions for two linked authored files, breakpoint binding in a
lifted helper, mapped paused helper and caller frames, stepping to the next
authored line, and mapped exception-path frames entered from a browser
microtask. It also confirmed that the lifted-call wrapper and an exported
compiler-generated record helper remain unmapped.

It also confirmed that the runtime carries `external_debug_info` but no
`.debug_*`, both artifacts validate, Chrome reports `ExternalDWARF`, and the
debug companion is absent from ordinary runtime-page requests.

The full UI showed the lifted helper's Wasm name
`$__waluau_top_level_init$lambda0`, its wrapper's generated name, and `$run`.
This is expected: with `DW_LANG_lo_user`, the official C/C++ extension maps
source locations but returns no DWARF function frame, so removing the Wasm
`name` section would regress call-stack usefulness. Wasm GC record locals in
the helper still cannot be serialized or expanded by the extension bridge.

The verifier uses Chrome for Testing because it honors temporary unpacked
extensions and isolated profiles. A stable-Chrome UI check remains the short,
explicit manual procedure in the fixture README; do not point automation at a
personal profile or claim that check without running it.

## Primary references

- [WebAssembly tool conventions: DWARF embedding and Code-section-relative addresses](https://github.com/WebAssembly/tool-conventions/blob/main/Dwarf.md)
- [Chrome DevTools: Debug C/C++ WebAssembly](https://developer.chrome.com/docs/devtools/wasm/)
- [Chrome DevTools language-extension API](https://chromium.googlesource.com/devtools/devtools-frontend/+/refs/heads/main/docs/language_extension_api.md)
- [Official C/C++ extension source](https://chromium.googlesource.com/devtools/devtools-frontend/+/refs/heads/main/extensions/cxx_debugging/)
- [DWARF language-code assignments](https://dwarfstd.org/languages.html)
