import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import { basename, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const WASM_BINDGEN_VERSION = '0.2.122'

function run(command, args, options = {}) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, { stdio: 'inherit', ...options })
    child.on('error', rejectRun)
    child.on('exit', (code) => {
      if (code === 0) {
        resolveRun()
        return
      }
      rejectRun(new Error(`${command} ${args.join(' ')} failed with exit code ${code}`))
    })
  })
}

async function ensureToolchain(workspaceRoot, logger) {
  await run('rustup', ['toolchain', 'install', 'stable', '--profile', 'minimal'], {
    cwd: workspaceRoot,
  })
  await run('rustup', ['target', 'add', 'wasm32-unknown-unknown', '--toolchain', 'stable'], {
    cwd: workspaceRoot,
  })

  try {
    await run('wasm-bindgen', ['--version'], { cwd: workspaceRoot, stdio: 'ignore' })
  } catch {
    logger.info(
      `Installing wasm-bindgen-cli v${WASM_BINDGEN_VERSION} (first run only)...`,
      { clear: false },
    )
    await run(
      'cargo',
      ['install', 'wasm-bindgen-cli', '--version', WASM_BINDGEN_VERSION, '--locked'],
      { cwd: workspaceRoot },
    )
  }
}

export async function buildWaluauWasm({ workspaceRoot, outDir, production, logger }) {
  const manifestPath = resolve(workspaceRoot, 'crates/waluau-wasm/Cargo.toml')
  const profile = production ? 'release' : 'debug'
  const wasmInput = resolve(
    workspaceRoot,
    `target/wasm32-unknown-unknown/${profile}/waluau_wasm.wasm`,
  )

  await ensureToolchain(workspaceRoot, logger)

  const cargoArgs = [
    'run',
    'stable',
    'cargo',
    'build',
    '--target',
    'wasm32-unknown-unknown',
    '--manifest-path',
    manifestPath,
  ]
  if (production) {
    cargoArgs.push('--release')
  }

  await run('rustup', cargoArgs, { cwd: workspaceRoot })
  await mkdir(outDir, { recursive: true })
  await run(
    'wasm-bindgen',
    [wasmInput, '--out-dir', outDir, '--target', 'web', '--no-typescript'],
    { cwd: workspaceRoot },
  )
}

export function waluauWasmPlugin(options = {}) {
  const playgroundRoot = fileURLToPath(new URL('.', import.meta.url))
  const workspaceRoot = options.workspaceRoot ?? resolve(playgroundRoot, '../..')
  const outDir = options.outDir ?? resolve(playgroundRoot, 'src/waluau-wasm')
  const production = options.production ?? false
  const buildWasm = options.buildWasm ?? buildWaluauWasm
  const fallbackLogger = {
    info: (message) => console.log(message),
    error: (message) => console.error(message),
  }

  let buildTimer
  let buildInFlight = null
  let queued = false
  let resolveInitialBuild
  let rejectInitialBuild
  const initialBuildReady = new Promise((resolveBuild, rejectBuild) => {
    resolveInitialBuild = resolveBuild
    rejectInitialBuild = rejectBuild
  })
  // buildStart's rejection is also observed by Vite. Keep this separate
  // readiness promise from becoming an unhandled rejection when no browser
  // request arrived before a failed startup build.
  void initialBuildReady.catch(() => {})

  async function rebuild(logger = fallbackLogger, { reloadServer } = {}) {
    if (buildInFlight) {
      queued = true
      return buildInFlight
    }

    logger.info('Building Waluau WASM compiler...', { clear: false })
    buildInFlight = buildWasm({ workspaceRoot, outDir, production, logger })
      .then(() => {
        if (reloadServer) {
          reloadServer.ws.send({ type: 'full-reload' })
        }
      })
      .finally(() => {
        buildInFlight = null
        if (queued) {
          queued = false
          void rebuild(logger, { reloadServer })
        }
      })

    return buildInFlight
  }

  return {
    name: 'waluau-wasm',
    enforce: 'pre',
    async buildStart() {
      try {
        await rebuild(this.environment?.logger ?? fallbackLogger)
        resolveInitialBuild()
      } catch (error) {
        rejectInitialBuild(error)
        throw error
      }
    },
    configureServer(server) {
      // Vite can begin accepting requests while buildStart is still producing
      // the initial wasm-bindgen output. Serving the page in that window lets
      // the browser import partial artifacts and permanently poisons the
      // module singleton. Hold requests until the compiler build is complete.
      server.middlewares.use((_request, _response, next) => {
        initialBuildReady.then(() => next(), next)
      })

      server.watcher.add(resolve(workspaceRoot, 'crates'))
      server.watcher.add(resolve(workspaceRoot, 'Cargo.lock'))

      const scheduleRebuild = (file) => {
        const name = basename(file)
        if (file.endsWith('.rs') || name === 'Cargo.toml' || name === 'Cargo.lock') {
          clearTimeout(buildTimer)
          buildTimer = setTimeout(() => {
            void rebuild(server.config.logger, { reloadServer: server })
          }, 150)
        }
      }

      server.watcher.on('add', scheduleRebuild)
      server.watcher.on('change', scheduleRebuild)
      server.watcher.on('unlink', scheduleRebuild)
    },
  }
}
