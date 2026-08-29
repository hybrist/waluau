import assert from 'node:assert/strict'
import test from 'node:test'

import { selectPlaygroundFunctions } from './src/hooks/playgroundExports.js'

test('generated main alias is hidden while a distinct authored main stays visible', () => {
  const exports = [
    { name: '__waluau_main', index: 7, params: [], results: [] },
    { name: 'main', index: 7, params: [], results: [] },
    { name: 'main', index: 11, params: [], results: ['i32'] },
    { name: 'answer', index: 12, params: [], results: ['i32'] },
  ]

  assert.deepEqual(
    selectPlaygroundFunctions(exports).map(func => [func.name, func.index]),
    [['main', 11], ['answer', 12]],
  )
})
