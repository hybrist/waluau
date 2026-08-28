// Ante-scale compiler performance benchmark (waluau-yzt1).
//
// Measures, against apps/ante at the current checkout:
//   1. cold      — full CLI builds in fresh processes: wall time, phase
//                  timings (WALUAU_TIMINGS), peak RSS (/usr/bin/time -l),
//                  and the workload stats from --report (source units,
//                  raw source bytes, post-link AST nodes).
//   2. lsp-clean — warm LSP edits: a valid whole-document change to the open
//                  Ante root, timed from didChange until the server has
//                  finished the analysis the change triggered.
//   3. lsp-error — the same, with an edit that introduces a type error.
//   4. lsp-multi — the valid edit with three Ante documents open (the LSP
//                  re-analyzes every open document per change).
//
// Timing boundaries: cold samples time the whole process (read through
// artifact write; no wasm-opt, no cargo). LSP samples time didChange-to-
// analysis-complete inside one long-lived server process, after a warmup
// edit, using a sentinel request as the completion barrier (see `barrier`).
//
// Usage: cargo build --release -p waluau-cli -p waluau-lsp && node tools/benchmark-ante.mjs
// Env: WALUAU_BENCH_COLD_SAMPLES (default 5), WALUAU_BENCH_LSP_SAMPLES
// (default 15), WALUAU_BENCH_JSON (path to also write the JSON result).

import { execSync, spawn } from 'node:child_process'
import { existsSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir, cpus, arch, platform, release } from 'node:os'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { performance } from 'node:perf_hooks'
import { mkdtempSync } from 'node:fs'

const workspace = resolve(import.meta.dirname, '..')
const compiler = join(workspace, 'target/release/waluau-cli')
const lspServer = join(workspace, 'target/release/waluau-lsp')
const entry = join(workspace, 'apps/ante/src/main.walu')
const coldSamples = Number(process.env.WALUAU_BENCH_COLD_SAMPLES ?? 5)
const lspSamples = Number(process.env.WALUAU_BENCH_LSP_SAMPLES ?? 15)

for (const binary of [compiler, lspServer]) {
  if (!existsSync(binary)) {
    throw new Error(`missing ${binary}; run: cargo build --release -p waluau-cli -p waluau-lsp`)
  }
}

const scratch = mkdtempSync(join(tmpdir(), 'waluau-bench-'))

function stats(samples) {
  const sorted = [...samples].sort((a, b) => a - b)
  const at = (q) => sorted[Math.min(sorted.length - 1, Math.ceil(q * sorted.length) - 1)]
  return {
    samples: sorted.map((v) => Number(v.toFixed(2))),
    median: Number(at(0.5).toFixed(2)),
    p95: Number(at(0.95).toFixed(2)),
    min: Number(sorted[0].toFixed(2)),
    max: Number(sorted.at(-1).toFixed(2)),
  }
}

// ---------------------------------------------------------------- cold builds

function coldBuild(index) {
  const report = join(scratch, `report-${index}.json`)
  const output = join(scratch, `ante-${index}.wasm`)
  const started = performance.now()
  const result = execSync(
    `/usr/bin/time -l ${JSON.stringify(compiler)} ${JSON.stringify(entry)} -o ${JSON.stringify(output)} --report ${JSON.stringify(report)} 2>&1`,
    { cwd: workspace, env: { ...process.env, WALUAU_TIMINGS: '1' }, encoding: 'utf8' },
  )
  const wallMs = performance.now() - started
  const rss = result.match(/(\d+)\s+maximum resident set size/)
  const footprint = result.match(/(\d+)\s+peak memory footprint/)
  const phases = {}
  const phaseLine = result.match(/^waluau timings: (.*)$/m)
  if (phaseLine) {
    for (const part of phaseLine[1].trim().split(/\s+/)) {
      const [name, value] = part.split('=')
      phases[name] = parseDuration(value)
    }
  }
  const reportJson = JSON.parse(readFileSync(report, 'utf8'))
  if (!reportJson.success) throw new Error(`cold build ${index} failed:\n${result}`)
  return {
    wallMs,
    peakRssBytes: rss ? Number(rss[1]) : null,
    peakFootprintBytes: footprint ? Number(footprint[1]) : null,
    phasesMs: phases,
    workload: reportJson.workload,
    involvedFiles: reportJson.involvedFiles,
  }
}

function parseDuration(text) {
  // Rust Debug for Duration: "5.96s", "913.83ms", "42ns", "1.35s"
  const match = text.match(/^([\d.]+)(ns|µs|ms|s)$/)
  if (!match) return null
  const value = Number(match[1])
  return { ns: value / 1e6, 'µs': value / 1e3, ms: value, s: value * 1e3 }[match[2]]
}

// ------------------------------------------------------------------ LSP client

function startLsp() {
  const child = spawn(lspServer, [], { cwd: workspace, stdio: ['pipe', 'pipe', 'inherit'] })
  let buffer = Buffer.alloc(0)
  const handlers = []
  child.stdout.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk])
    while (true) {
      const headerEnd = buffer.indexOf('\r\n\r\n')
      if (headerEnd === -1) return
      const header = buffer.subarray(0, headerEnd).toString()
      const length = Number(header.match(/Content-Length:\s*(\d+)/i)?.[1])
      if (buffer.length < headerEnd + 4 + length) return
      const body = buffer.subarray(headerEnd + 4, headerEnd + 4 + length).toString()
      buffer = buffer.subarray(headerEnd + 4 + length)
      const message = JSON.parse(body)
      for (const handler of [...handlers]) handler(message)
    }
  })
  let requestId = 0
  return {
    child,
    send(method, params, isRequest) {
      const message = { jsonrpc: '2.0', method, params }
      if (isRequest) message.id = ++requestId
      const body = JSON.stringify(message)
      child.stdin.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`)
      return message.id
    },
    wait(predicate) {
      return new Promise((resolveWait) => {
        const handler = (message) => {
          if (predicate(message)) {
            handlers.splice(handlers.indexOf(handler), 1)
            resolveWait(message)
          }
        }
        handlers.push(handler)
      })
    },
    watch(handler) {
      handlers.push(handler)
    },
    unwatch(handler) {
      const index = handlers.indexOf(handler)
      if (index !== -1) handlers.splice(index, 1)
    },
  }
}

const uriOf = (path) => pathToFileURL(path).href

// The server handles messages sequentially and publishes diagnostics while
// processing the change notification itself (a clean analysis publishes
// nothing at all), so completion is observed with a sentinel request sent
// straight after the notification: its response arrives exactly when the
// analysis triggered by the change has finished.
async function barrier(lsp, notifyMethod, params) {
  const errorDiagnostics = []
  const collect = (message) => {
    if (message.method === 'textDocument/publishDiagnostics' && message.params.diagnostics.length > 0) {
      errorDiagnostics.push(...message.params.diagnostics)
    }
    return false
  }
  lsp.watch(collect)
  const started = performance.now()
  lsp.send(notifyMethod, params)
  const sentinelId = lsp.send('waluau/benchmarkSync', {}, true)
  await lsp.wait((message) => message.id === sentinelId)
  const elapsed = performance.now() - started
  lsp.unwatch(collect)
  return { elapsed, errorDiagnostics }
}

async function openDocument(lsp, path) {
  await barrier(lsp, 'textDocument/didOpen', {
    textDocument: { uri: uriOf(path), languageId: 'waluau', version: 1, text: readFileSync(path, 'utf8') },
  })
}

async function timedEdit(lsp, path, text, { expectClean }) {
  const { elapsed, errorDiagnostics } = await barrier(lsp, 'textDocument/didChange', {
    textDocument: { uri: uriOf(path), version: ++documentVersion },
    contentChanges: [{ text }],
  })
  if (expectClean !== (errorDiagnostics.length === 0)) {
    throw new Error(
      `expected ${expectClean ? 'clean' : 'error'} diagnostics for ${path}, got ${JSON.stringify(errorDiagnostics.slice(0, 3))}`,
    )
  }
  return elapsed
}

let documentVersion = 1

async function lspScenarios() {
  const lsp = startLsp()
  const initialized = lsp.wait((message) => message.id === 1 && message.result)
  lsp.send('initialize', { processId: null, rootUri: uriOf(workspace), capabilities: {} }, true)
  await initialized
  lsp.send('initialized', {})

  const mainSource = readFileSync(entry, 'utf8')
  await openDocument(lsp, entry)

  // Warmup edit so every timed sample runs against a warm session.
  await timedEdit(lsp, entry, `${mainSource}\n-- warmup\n`, { expectClean: true })

  const clean = []
  for (let index = 0; index < lspSamples; index += 1) {
    clean.push(await timedEdit(lsp, entry, `${mainSource}\n-- edit ${index}\n`, { expectClean: true }))
  }

  const errors = []
  for (let index = 0; index < lspSamples; index += 1) {
    const bad = `${mainSource}\nconst BENCH_BAD_${index}: i32 = "not a number"\n`
    errors.push(await timedEdit(lsp, entry, bad, { expectClean: false }))
    await timedEdit(lsp, entry, `${mainSource}\n-- recovered ${index}\n`, { expectClean: true })
  }

  // Multi-root: three Ante documents open; every change re-analyzes each.
  const extraRoots = [join(workspace, 'apps/ante/src/game.walu'), join(workspace, 'apps/ante/src/flow.walu')]
  for (const extra of extraRoots) await openDocument(lsp, extra)
  await timedEdit(lsp, entry, `${mainSource}\n-- multi warmup\n`, { expectClean: true })
  const multi = []
  for (let index = 0; index < lspSamples; index += 1) {
    multi.push(await timedEdit(lsp, entry, `${mainSource}\n-- multi ${index}\n`, { expectClean: true }))
  }

  lsp.send('shutdown', {}, true)
  lsp.send('exit', {})
  lsp.child.stdin.end()
  return { clean, errors, multi }
}

// ---------------------------------------------------------------------- main

const cold = []
for (let index = 0; index < coldSamples; index += 1) cold.push(coldBuild(index))

const workload = cold[0].workload
const appFiles = cold[0].involvedFiles
const appSourceBytes = appFiles.reduce((sum, file) => sum + statSync(file).size, 0)
const { clean, errors, multi } = await lspScenarios()

const coldWall = stats(cold.map((sample) => sample.wallMs))
const result = {
  meta: {
    commit: execSync('git rev-parse --short HEAD', { cwd: workspace, encoding: 'utf8' }).trim(),
    date: new Date().toISOString(),
    host: { platform: platform(), release: release(), arch: arch(), cpu: cpus()[0].model, cores: cpus().length },
    profile: 'release',
    coldSamples,
    lspSamples,
  },
  workload: {
    sourceUnits: workload.sourceUnits,
    appSourceBytes,
    linkedSourceBytes: workload.linkedSourceBytes,
    astNodes: workload.astNodes,
  },
  cold: {
    wallMs: coldWall,
    throughputMBps: Number((appSourceBytes / 1e6 / (coldWall.median / 1e3)).toFixed(3)),
    peakRssBytes: stats(cold.map((sample) => sample.peakRssBytes)),
    peakFootprintBytes: stats(cold.map((sample) => sample.peakFootprintBytes)),
    phasesMsLastSample: cold.at(-1).phasesMs,
  },
  lspCleanEditMs: stats(clean),
  lspErrorEditMs: stats(errors),
  lspMultiRootCleanEditMs: stats(multi),
}

const rendered = JSON.stringify(result, null, 2)
console.log(rendered)
if (process.env.WALUAU_BENCH_JSON) writeFileSync(process.env.WALUAU_BENCH_JSON, rendered)
