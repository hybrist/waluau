import { createHash } from 'node:crypto'
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { spawn } from 'node:child_process'
import { performance } from 'node:perf_hooks'

const workspace = resolve(import.meta.dirname, '..')
const compiler = join(workspace, 'target/release/waluau-cli')
const iterations = Number(process.env.WALUAU_BENCH_ITERATIONS ?? 10)
const thresholdMs = Number(process.env.WALUAU_BENCH_THRESHOLD_MS ?? 100)

if (!existsSync(compiler)) {
  throw new Error(`missing ${compiler}; run cargo build --release -p waluau-cli first`)
}

const temporary = mkdtempSync(join(tmpdir(), 'waluau-ante-incremental-'))
const sourceDir = join(temporary, 'src')
cpSync(join(workspace, 'apps/ante/src'), sourceDir, { recursive: true })
const entry = join(sourceDir, 'main.walu')
const editedFile = join(sourceDir, 'spell_cast.walu')
const output = join(temporary, 'ante.wasm')
const compilerProcess = spawn(compiler, ['--server'], {
  cwd: workspace,
  stdio: ['pipe', 'pipe', 'inherit'],
})

let stdout = ''
const responses = []
compilerProcess.stdout.setEncoding('utf8')
compilerProcess.stdout.on('data', (chunk) => {
  stdout += chunk
  let newline
  while ((newline = stdout.indexOf('\n')) !== -1) {
    const line = stdout.slice(0, newline)
    stdout = stdout.slice(newline + 1)
    responses.shift()?.(JSON.parse(line))
  }
})

function build(id) {
  return new Promise((resolveResponse, reject) => {
    responses.push((response) => {
      if (!response.ok) reject(new Error(response.error))
      else resolveResponse(response)
    })
    compilerProcess.stdin.write(`${JSON.stringify({ id, args: [entry, '-o', output] })}\n`)
  })
}

function toggleLiteral() {
  const source = readFileSync(editedFile, 'utf8')
  const from = source.includes('0::i32, 32::i32') ? '0::i32, 32::i32' : '0::i32, 33::i32'
  const to = from.endsWith('32::i32') ? '0::i32, 33::i32' : '0::i32, 32::i32'
  if (!source.includes(from)) throw new Error('Ante benchmark literal was not found')
  writeFileSync(editedFile, source.replace(from, to))
}

try {
  await build(0)
  const samples = []
  const hashes = new Set()
  for (let index = 0; index < iterations; index += 1) {
    toggleLiteral()
    const started = performance.now()
    await build(index + 1)
    samples.push(performance.now() - started)
    hashes.add(createHash('sha256').update(readFileSync(output)).digest('hex'))
  }
  samples.sort((left, right) => left - right)
  const median = samples[Math.floor(samples.length / 2)]
  console.log(JSON.stringify({
    medianMs: Number(median.toFixed(2)),
    minMs: Number(samples[0].toFixed(2)),
    maxMs: Number(samples.at(-1).toFixed(2)),
    samplesMs: samples.map((sample) => Number(sample.toFixed(2))),
    distinctArtifacts: hashes.size,
    thresholdMs,
  }, null, 2))
  if (hashes.size !== 2) throw new Error(`expected two stable artifact hashes, observed ${hashes.size}`)
  if (median >= thresholdMs) process.exitCode = 1
} finally {
  compilerProcess.stdin.end()
  rmSync(temporary, { recursive: true, force: true })
}
