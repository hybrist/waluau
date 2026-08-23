# Chrome Wasm GC DWARF probe

This fixture isolates the contract between a future Waluau DWARF emitter and
Chrome DevTools. `dwarf_chrome_probe.wasm` is a browser module containing:

- authored line mappings and subprogram DIEs for `dwarf_chrome_probe.walu`;
- embedded DWARF 4 sections produced by LLVM;
- a `DW_LANG_lo_user` compile unit, matching the Waluau emission policy;
- an extra Wasm GC struct and round-trip function with no DWARF mapping; and
- trapping exports for comparing paused call frames with JavaScript stack text.

The checked-in binary is a compatibility fixture, not compiler output. The
builder compiles a line-aligned C carrier solely to obtain conventional DWARF,
then appends a GC type and function directly to the binary without rewriting
the existing Code-section bytes. Clang and `wasm-ld` are native build tools;
the resulting program is instantiated only in a browser.

## Rebuild and inspect

LLVM 21 and `wasm-tools` 1.252.0 produced the checked-in artifact. Point the
variables at an LLVM installation with the WebAssembly target:

```sh
WALUAU_DWARF_CLANG=/path/to/clang \
WALUAU_DWARF_WASM_LD=/path/to/wasm-ld \
node fixtures/dwarf-chrome-wasm-gc/build-probe.mjs

wasm-tools validate --features all \
  fixtures/dwarf-chrome-wasm-gc/dwarf_chrome_probe.wasm
wasm-tools objdump fixtures/dwarf-chrome-wasm-gc/dwarf_chrome_probe.wasm
wasm-tools addr2line --code-section-relative \
  fixtures/dwarf-chrome-wasm-gc/dwarf_chrome_probe.wasm 31 56 109 162
```

Offsets 31, 56, and 109 resolve to authored Waluau statements. Offset 162 is
inside the intentionally unmapped synthetic GC helper and must not resolve.

## Verify in Chrome DevTools

1. Install Google's
   [C/C++ DevTools Support (DWARF) extension](https://chromewebstore.google.com/detail/cc-devtools-support-dwarf/pdcpmagijalfljmkmjngeonclgbbannb).
2. Start `node fixtures/dwarf-chrome-wasm-gc/serve-probe.mjs` and open the
   printed runtime URL in current stable Chrome.
3. Open DevTools before reloading. In Sources, open
   `dwarf_chrome_probe.walu` and set a line breakpoint on line 4.
4. Click **Run mapped call**. The breakpoint must bind and pause at line 4;
   the next step-over must stop at line 5. The paused Call Stack must map the
   `inner` and `run` frames to this Waluau file.
5. Click **Log caught error**. Compare the Console rendering with
   `lastProbeError.stack`; both are expected to retain Wasm function names and
   byte offsets rather than being rewritten through DWARF.
6. Enable pause on uncaught exceptions and click **Throw uncaught error**.
   Confirm the paused debugger maps the authored frames, while the uncaught
   Console presentation remains a raw Wasm stack.
7. The initial page output must include `GC round trip = 42`. The exported
   `__synthetic_gc_round_trip` function deliberately has no authored mapping;
   debugging it falls back to the raw Wasm view.

## Wasm GC locals limitation

The synthetic helper constructs a GC struct, but no pass is claimed for
inspecting that reference as a local or expanding the aggregate. The official
extension's host bridge serializes `i32`, `i64`, `f32`, `f64`, and `v128`; it
throws `cannot serialize non-numerical wasm type` for a reference value. Its
object protocol also describes C/C++ values through linear-memory addresses,
which cannot traverse a Wasm GC object. Revisit this check when Chrome's
language-extension API can carry reference values, then give a mapped authored
function a live struct local and inspect it in Scope while paused.

## Exercise the official extension parser directly

For a non-UI compatibility check, unpack the official extension CRX and pass
the directory containing `DevToolsPluginHost.bundle.js`:

```sh
node fixtures/dwarf-chrome-wasm-gc/serve-probe.mjs /path/to/unpacked-extension
```

Open the printed extension-parser URL. The JSON result verifies source
discovery, mapped lines, both mapping directions, function-query behavior, and
the absence of a synthetic-helper mapping by using the extension's exact
worker and symbols engine. With `DW_LANG_lo_user`, an empty authored-function
frame is expected; DevTools retains the raw name from the Wasm `name` section.
