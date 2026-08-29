/**
 * Functions the playground presents for interactive calls.
 *
 * The compiler may emit `main` as a compatibility alias for its generated
 * initialization function. Function identity, not spelling, distinguishes
 * that alias from an authored `export function main`.
 */
export function selectPlaygroundFunctions(wasmExports, generatedMainName = '__waluau_main') {
  const generatedMain = wasmExports.find(func => func.name === generatedMainName);
  return wasmExports
    .filter(func => !func.name.startsWith('__waluau'))
    .filter(func => !(func.name === 'main' && func.index === generatedMain?.index));
}
