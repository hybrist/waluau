import assert from 'node:assert/strict'
import test from 'node:test'

import { waluauWasmPlugin } from './vite-plugin-waluau-wasm.js'

function deferred() {
  let resolve
  const promise = new Promise((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

test('dev requests wait for the initial wasm compiler build', async () => {
  const build = deferred()
  let buildStarted = false
  const plugin = waluauWasmPlugin({
    buildWasm: async () => {
      buildStarted = true
      await build.promise
    },
  })
  const middleware = []
  const logger = { info() {}, error() {} }
  plugin.configureServer({
    config: { logger },
    middlewares: { use(handler) { middleware.push(handler) } },
    watcher: { add() {}, on() {} },
  })

  assert.equal(middleware.length, 1, 'the initial-build gate should be registered')
  let nextCalls = 0
  let nextError
  middleware[0]({}, {}, (error) => {
    nextCalls += 1
    nextError = error
  })
  await Promise.resolve()
  assert.equal(nextCalls, 0, 'a request before buildStart must remain blocked')

  const buildStart = plugin.buildStart.call({ environment: { logger } })
  await Promise.resolve()
  assert.equal(buildStarted, true)
  assert.equal(nextCalls, 0, 'a request during the wasm build must remain blocked')

  build.resolve()
  await buildStart
  await Promise.resolve()
  assert.equal(nextCalls, 1, 'the request should continue after the wasm build')
  assert.equal(nextError, undefined)
})
