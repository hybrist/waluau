// Shared loader for the waluau wasm compiler module.
//
// The wasm-bindgen init function only guards against *sequential* double
// initialization (`if (wasm !== undefined) return`). Two callers racing the
// first load — e.g. the compiler and REPL hooks mounting together — would
// both instantiate the module, and the second instantiation swaps the
// module-level `wasm` binding out from under objects created against the
// first instance. Long-lived bindgen objects (the language server) then
// dereference another instance's memory, surfacing as "recursive use of an
// object detected which would lead to unsafe aliasing in rust". Memoizing
// the whole load-and-init as one promise makes initialization happen exactly
// once no matter how many callers race.
let modulePromise = null;

export function loadWaluauWasm() {
  if (!modulePromise) {
    modulePromise = import('../waluau-wasm/waluau_wasm.js').then(async (module) => {
      await module.default();
      return module;
    });
  }
  return modulePromise;
}
