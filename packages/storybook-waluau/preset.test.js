import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { core, experimental_indexers, viteFinal } from './preset.js';

const presetsWith = (framework) => ({ apply: async (key) => (key === 'framework' ? framework : {}) });

test('builds with Vite and renders with the Waluau renderer', () => {
  assert.match(core.builder, /@storybook\/builder-vite/);
  assert.match(core.renderer, /renderer-preset\.js$/);
});

test('indexes every story a .stories.walu file publishes', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-storybook-'));
  try {
    const file = join(root, 'card.stories.walu');
    await writeFile(file, `local storybook = require("waluau:engine/storybook")
storybook.publish({
    storybook.story("Face up", { draw = draw_face_up }),
    storybook.story("Face down", { draw = draw_face_down }),
})`);

    const [indexer, ...rest] = await experimental_indexers([{ test: /\.mdx$/ }]);
    assert.ok(indexer.test.test(file));
    assert.equal(rest.length, 1);

    const entries = await indexer.createIndex(file, { makeTitle: () => 'card' });
    assert.deepEqual(entries, [
      { type: 'story', importPath: file, exportName: 'FaceUp', name: 'Face up', title: 'card' },
      { type: 'story', importPath: file, exportName: 'FaceDown', name: 'Face down', title: 'card' },
    ]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('compiles stories with the options in framework.options.waluau', async () => {
  const config = await viteFinal({ plugins: [] }, {
    presets: presetsWith({
      name: '@waluau/storybook',
      options: { waluau: { manifest: 'waluau.assets.json' } },
    }),
  });

  const [plugin] = config.plugins;
  assert.equal(plugin.name, 'waluau-game');
  // Storybook's preview page is not the game's page: no viewport takeover.
  assert.deepEqual(plugin.transformIndexHtml(), []);
});

test("replaces an app's own game plugin instead of stacking a second one", async () => {
  const appPlugin = { name: 'waluau-game', transformIndexHtml: () => ['full-screen styles'] };
  const config = await viteFinal({ plugins: [[appPlugin], { name: 'other' }] }, {
    presets: presetsWith('@waluau/storybook'),
  });

  assert.deepEqual(config.plugins.map(({ name }) => name), ['other', 'waluau-game']);
  assert.notEqual(config.plugins.at(-1), appPlugin);
});
