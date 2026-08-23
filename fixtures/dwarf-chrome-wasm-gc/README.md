# Chrome Wasm GC DWARF probe

This fixture covers Waluau compiler output and the lower-level contract between
embedded DWARF and Chrome DevTools. `dwarf_chrome_probe.wasm` is a checked-in
compatibility module containing:

- authored line mappings and subprogram DIEs for `dwarf_chrome_probe.walu`;
- embedded DWARF 4 sections produced by LLVM;
- a `DW_LANG_lo_user` compile unit, matching the Waluau emission policy;
- an extra Wasm GC struct and round-trip function with no DWARF mapping; and
- trapping exports for comparing paused call frames with JavaScript stack text.

The checked-in binary is not compiler output. The builder compiles a
line-aligned C carrier solely to obtain conventional DWARF, then appends a GC
type and function directly to the binary without rewriting existing
Code-section bytes. Clang and `wasm-ld` are native build tools; the resulting
program is instantiated only in a browser.

## Rebuild and inspect the compatibility module

LLVM 21 and `wasm-tools` 1.252.0 produced the checked-in artifact. Point the
variables at an LLVM installation with the WebAssembly target:

```sh
WALUAU_DWARF_CLANG=/path/to/clang \
WALUAU_DWARF_WASM_LD=/path/to/wasm-ld \
node fixtures/dwarf-chrome-wasm-gc/build-probe.mjs

wasm-tools validate --features all \
  fixtures/dwarf-chrome-wasm-gc/dwarf_chrome_probe.wasm
wasm-tools addr2line --code-section-relative \
  fixtures/dwarf-chrome-wasm-gc/dwarf_chrome_probe.wasm 31 56 109 162
```

Offsets 31, 56, and 109 resolve to authored Waluau statements. Offset 162 is
inside the intentionally unmapped synthetic GC helper and must not resolve.

## Verify actual compiler output in Chrome

The opt-in verifier builds `compiler_probe_main.walu` and its required
`compiler_probe_helper.walu` twice with the real CLI. It asserts that default
output has no debug reference, development runtime output contains only an
`external_debug_info` reference, and the sibling contains all DWARF sections.
It measures the runtime reference and companion separately, runs
the official extension worker, and drives the full DevTools model. It uses a
fresh temporary profile and requires explicit paths; it does not install a
browser, download or commit an extension, or touch a personal Chrome profile.

Install repository JavaScript dependencies, unpack Google's extension into a
scratch directory, and point the verifier at Chrome for Testing (recommended):

```sh
pnpm install
pnpm --filter ante exec node \
  ../../fixtures/dwarf-chrome-wasm-gc/verify-compiler-chrome.mjs \
  --chrome "/path/to/Google Chrome for Testing" \
  --extension /path/to/unpacked-cxx-devtools-extension
```

Add `--headed` to watch the run. The script checks:

- both authored files are discovered and map in both directions;
- a breakpoint binds in the lifted helper, and paused helper and caller frames
  map across the two files;
- step-over reaches the next authored line;
- an exception path entered from a queued browser microtask maps its authored
  helper and caller, while its generated wrapper remains in the raw Wasm view;
- a compiler-generated record helper has no source or DWARF function mapping;
- the Wasm `name` section remains in runtime output;
- an ordinary runtime page load never requests the debug companion; and
- Console/error stack objects are never rewritten to `.walu` locations.

The JSON report includes every mapping, raw function name, stack value, and the
exact size delta. CI does not download Chrome or the extension, so this is an
explicit browser compatibility check rather than a default test.

To build the compiler fixture without launching Chrome:

```sh
cargo run -p waluau-cli -- \
  fixtures/dwarf-chrome-wasm-gc/compiler_probe_main.walu \
  -o fixtures/dwarf-chrome-wasm-gc/compiler_dwarf_probe.wasm \
  --emit-js --development-dwarf

wasm-tools validate --features all \
  fixtures/dwarf-chrome-wasm-gc/compiler_dwarf_probe.wasm
wasm-tools validate --features all \
  fixtures/dwarf-chrome-wasm-gc/compiler_dwarf_probe.debug.wasm
```

The generated Wasm and JavaScript are ignored build artifacts. After starting
the server, open `/fixture/compiler-probe.html` for the runtime UI.

### Manual stable-Chrome check

1. Install Google's
   [C/C++ DevTools Support (DWARF) extension](https://chromewebstore.google.com/detail/cc-devtools-support-dwarf/pdcpmagijalfljmkmjngeonclgbbannb).
2. Build the compiler fixture above, start
   `node fixtures/dwarf-chrome-wasm-gc/serve-probe.mjs`, and open
   `/fixture/compiler-probe.html` in current stable Chrome.
3. Open DevTools before reloading.
4. Open `compiler_probe_helper.walu` and set a line breakpoint on line 4.
   Click **Run mapped call**. The breakpoint must bind and pause there; stepping
   over the expression reaches line 5. The paused Call Stack maps the helper to
   that file and `run` to `compiler_probe_main.walu` line 4. The lifted helper's
   displayed function name can remain its generated name from the Wasm `name`
   section; source mapping and frame naming are separate contracts.
5. Click **Log caught error**. Compare the Console rendering with
   `lastProbeError.stack`. DWARF does not rewrite either surface. When Chrome
   supplies stack strings they retain name-section function names and Wasm
   offsets; some Chrome versions expose no `.stack` for Waluau's tagged
   `WebAssembly.Exception`.
6. Set a breakpoint on `compiler_probe_helper.walu` line 9 and click **Throw
   uncaught error**. The browser event queues the call through a microtask. The
   authored helper and caller map, while the generated wrapper stays raw. After
   continuing, the uncaught Console presentation is likewise not source-mapped.

The checked-in carrier remains useful for a raw `unreachable` trap with stack
strings and an invokable synthetic GC function. Its runtime page is
`/fixture/probe.html`; `__synthetic_gc_round_trip(42)` returns 42 and offset 162
must remain unmapped.

## Wasm GC locals limitation

The compiler probe steps to line 5 after its lifted helper materializes a
`ProbeBox` Wasm GC record local, but no pass is claimed for inspecting that
reference or expanding the aggregate. The official extension's host bridge
serializes `i32`, `i64`, `f32`, `f64`, and `v128`; it throws
`cannot serialize non-numerical wasm type`
for a reference value. Its object protocol also describes C/C++ values through
linear-memory addresses, which cannot traverse a Wasm GC object. Revisit this
check when Chrome's language-extension API can carry reference values.

## Exercise the official extension parser directly

For a non-UI compatibility check, unpack the official extension CRX and pass
the directory containing `DevToolsPluginHost.bundle.js`:

```sh
node fixtures/dwarf-chrome-wasm-gc/serve-probe.mjs /path/to/unpacked-extension
```

Open the printed extension-parser URL. Query parameters select a module and
repeat expected sources, for example:

```text
/extension/extension-harness.html?module=compiler_dwarf_probe.wasm&symbols=compiler_dwarf_probe.debug.wasm&source=compiler_probe_main.walu&source=compiler_probe_helper.walu&syntheticOffset=298
```

The JSON result verifies source discovery, mapped lines, both mapping
directions, function-query behavior, and the absence of a synthetic-helper
mapping by using the extension's exact worker and symbols engine. The verifier
derives the compiler helper offset rather than relying on the example value.
With `DW_LANG_lo_user`, an empty authored-function frame is expected; DevTools
retains the raw name from the Wasm `name` section.
