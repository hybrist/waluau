import assert from 'node:assert/strict';
import { test } from 'node:test';

import { renderToCanvas } from './entry-preview.js';

test('mounts a generated story with its current Storybook args', async () => {
  const calls = [];
  const book = {
    mount: async (...args) => calls.push(['mount', ...args]),
    unmount: () => calls.push(['unmount']),
  };
  const canvas = {};
  const args = { suit: 3, rank: 2 };
  let shown = false;

  const teardown = await renderToCanvas({
    storyFn: () => ({ book, name: 'Face up', args }),
    name: 'Face up',
    showMain: () => { shown = true; },
    showError: () => assert.fail('the generated story should be mountable'),
  }, canvas);

  assert.deepEqual(calls, [['mount', 'Face up', canvas, args]]);
  assert.equal(shown, true);
  teardown();
  assert.deepEqual(calls.at(-1), ['unmount']);
});
