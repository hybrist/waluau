// Vite plugin that turns *.test.walu files into runnable vitest modules.
//
// Each matched file becomes a JS module that compiles the Waluau source in
// the browser (via the app's built waluau-wasm compiler), instantiates it
// with the walu-test host bridge, and runs its top level — which registers
// the file's describe/it suites with vitest during collection.
import { readFile } from 'node:fs/promises'
import { dirname, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HOST_MODULE_PATH = fileURLToPath(new URL('./host.js', import.meta.url))

// Imports in the generated module are emitted relative to the test file so
// Vite resolves them like hand-written relative imports (the monorepo's
// workspace root keeps them inside the allowed fs scope).
function importSpecifier(fromFile, target) {
  const specifier = relative(dirname(fromFile), target).split('\\').join('/')
  return specifier.startsWith('.') ? specifier : `./${specifier}`
}

export function waluTestPlugin(options = {}) {
  const waluauWasmPath = options.waluauWasmPath
  if (!waluauWasmPath) {
    throw new Error(
      'waluTestPlugin requires a waluauWasmPath option pointing at the built waluau_wasm.js module',
    )
  }
  const resolvedWasmPath = resolve(waluauWasmPath)

  return {
    name: 'walu-test',
    enforce: 'pre',
    async load(id) {
      const [filePath] = id.split('?', 1)
      if (!filePath.endsWith('.test.walu')) {
        return null
      }
      const source = await readFile(filePath, 'utf8')
      const hostSpecifier = importSpecifier(filePath, HOST_MODULE_PATH)
      const wasmSpecifier = importSpecifier(filePath, resolvedWasmPath)
      const entryName = filePath.split('/').pop()
      return [
        `import init, { compile_multi } from ${JSON.stringify(wasmSpecifier)};`,
        `import { registerWaluTests } from ${JSON.stringify(hostSpecifier)};`,
        `const source = ${JSON.stringify(source)};`,
        `await registerWaluTests({ source, path: ${JSON.stringify(`/${entryName}`)}, init, compile_multi });`,
        '',
      ].join('\n')
    },
  }
}
