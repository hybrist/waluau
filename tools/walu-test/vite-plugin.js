// Vite plugin that turns *.test.walu files into runnable vitest modules by
// compiling them in the browser with the app's built waluau-wasm compiler.
//
// Most apps should use the waluau() plugin from @waluau/vite-plugin instead,
// which compiles test files ahead of time with the native CLI. This variant
// exists for apps that ship the wasm compiler (e.g. the conformance runner)
// and want test compilation to exercise that in-browser pipeline.
import { readFile } from 'node:fs/promises'
import { dirname, relative, resolve } from 'node:path'

// Imports in the generated module are emitted relative to the test file so
// Vite resolves them like hand-written relative imports; the bare
// '@waluau/vite-plugin/testing' specifier resolves through the consuming
// app's node_modules.
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
      const wasmSpecifier = importSpecifier(filePath, resolvedWasmPath)
      const entryName = filePath.split('/').pop()
      return [
        `import init, { compile_multi } from ${JSON.stringify(wasmSpecifier)};`,
        `import { registerWaluTests } from '@waluau/vite-plugin/testing';`,
        `const source = ${JSON.stringify(source)};`,
        `await registerWaluTests({ source, path: ${JSON.stringify(`/${entryName}`)}, init, compile_multi });`,
        '',
      ].join('\n')
    },
  }
}
