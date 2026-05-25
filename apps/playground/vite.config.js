import { spawn } from 'node:child_process'
import { mkdir, copyFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { basename, resolve } from 'node:path'

import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const rootDir = fileURLToPath(new URL('.', import.meta.url))
const workspaceRoot = resolve(rootDir, '../..')
const wasmManifestPath = resolve(workspaceRoot, 'crates/waluau-wasm/Cargo.toml')
const builtWasmPath = resolve(
  workspaceRoot,
  'target/wasm32-unknown-unknown/debug/waluau_wasm.wasm'
)
const playgroundWasmPath = resolve(rootDir, 'public/waluau_wasm.wasm')

function wasmDevPlugin() {
  let buildTimer
  let buildInFlight = null
  let queued = false

  async function copyBuiltWasm() {
    await mkdir(resolve(rootDir, 'public'), { recursive: true })
    await copyFile(builtWasmPath, playgroundWasmPath)
  }

  function runWasmBuild(server) {
    if (buildInFlight) {
      queued = true
      return buildInFlight
    }

    server.config.logger.info('Rebuilding playground WASM...', { clear: false })
    buildInFlight = new Promise((resolveBuild, rejectBuild) => {
      const child = spawn(
        'rustup',
        [
          'run',
          'stable',
          'cargo',
          'build',
          '--target',
          'wasm32-unknown-unknown',
          '--manifest-path',
          wasmManifestPath,
        ],
        {
          cwd: workspaceRoot,
          stdio: 'inherit',
        }
      )

      child.on('error', rejectBuild)
      child.on('exit', async (code) => {
        if (code !== 0) {
          rejectBuild(new Error(`WASM build failed with exit code ${code}`))
          return
        }

        try {
          await copyBuiltWasm()
          server.ws.send({ type: 'full-reload' })
          resolveBuild()
        } catch (error) {
          rejectBuild(error)
        }
      })
    })

    buildInFlight
      .catch((error) => {
        server.config.logger.error(error.message, { timestamp: true })
      })
      .finally(() => {
        buildInFlight = null
        if (queued) {
          queued = false
          void runWasmBuild(server)
        }
      })

    return buildInFlight
  }

  function scheduleBuild(server) {
    clearTimeout(buildTimer)
    buildTimer = setTimeout(() => {
      void runWasmBuild(server)
    }, 150)
  }

  return {
    name: 'playground-wasm-dev',
    configureServer(server) {
      server.watcher.add(resolve(workspaceRoot, 'crates'))
      server.watcher.add(resolve(workspaceRoot, 'Cargo.lock'))

      const maybeRebuild = (file) => {
        const name = basename(file)
        if (file.endsWith('.rs') || name === 'Cargo.toml' || name === 'Cargo.lock') {
          scheduleBuild(server)
        }
      }

      server.watcher.on('add', maybeRebuild)
      server.watcher.on('change', maybeRebuild)
      server.watcher.on('unlink', maybeRebuild)

      void runWasmBuild(server)
    },
  }
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), wasmDevPlugin()],
  server: {
    fs: {
      allow: [workspaceRoot],
    },
  },
})
