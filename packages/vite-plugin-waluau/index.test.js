import assert from 'node:assert/strict';
import { test } from 'node:test';

import { waluau } from './index.js';

test('resolves the generated game entry', () => {
  const plugin = waluau({ compiler: { command: 'true' } });
  assert.equal(plugin.resolveId('virtual:waluau-game'), '\0virtual:waluau-game');
  assert.equal(plugin.resolveId('other'), null);
});

test('takes over the viewport by default', () => {
  const plugin = waluau({ compiler: { command: 'true' } });
  const transformed = plugin.transformIndexHtml.handler();
  const style = transformed.find(({ tag }) => tag === 'style');
  const script = transformed.find(({ tag }) => tag === 'script');
  assert.match(style.children, /100vw !important/);
  assert.equal(script.children, 'import "virtual:waluau-game";');
});

test('allows embedding without full-screen styles', () => {
  const plugin = waluau({ fullScreen: false, compiler: { command: 'true' } });
  const transformed = plugin.transformIndexHtml.handler();
  assert.equal(transformed.some(({ tag }) => tag === 'style'), false);
});
