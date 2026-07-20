import assert from 'node:assert/strict';
import { test } from 'node:test';

import { createWaluauShaderSourceHost } from './shaders.js';

test('serves production shader sources and revisioned development updates', () => {
  const host = createWaluauShaderSourceHost({
    vertex: 'initial vertex',
    pixel: 'initial pixel',
  });
  const imports = host.imports;

  assert.equal(imports.__waluau_shader_source_revision('vertex'), 1);
  assert.equal(imports.__waluau_shader_source_text('vertex'), 'initial vertex');
  assert.equal(imports.__waluau_shader_source_revision('pixel'), 1);

  assert.equal(host.update('pixel', 'updated pixel'), 2);
  assert.equal(imports.__waluau_shader_source_revision('vertex'), 1);
  assert.equal(imports.__waluau_shader_source_revision('pixel'), 2);
  assert.equal(imports.__waluau_shader_source_text('pixel'), 'updated pixel');
  assert.equal(host.update('pixel', 'updated pixel'), 2);
});

test('rejects invalid updates without corrupting the last source', () => {
  const host = createWaluauShaderSourceHost({ pixel: 'valid source' });

  assert.throws(() => host.update('missing', 'new source'), /Unknown/);
  assert.throws(() => host.update('pixel', undefined), /must be a string/);
  assert.equal(host.imports.__waluau_shader_source_revision('pixel'), 1);
  assert.equal(host.imports.__waluau_shader_source_text('pixel'), 'valid source');
});

test('reports missing sources without throwing through the Wasm import boundary', () => {
  const imports = createWaluauShaderSourceHost().imports;

  assert.equal(imports.__waluau_shader_source_revision('missing'), -1);
  assert.equal(imports.__waluau_shader_source_text('missing'), '');
});

test('preserves __proto__ as an ordinary logical source name', () => {
  const host = createWaluauShaderSourceHost(
    Object.fromEntries([['__proto__', 'prototype shader']]),
  );

  assert.equal(host.imports.__waluau_shader_source_revision('__proto__'), 1);
  assert.equal(host.imports.__waluau_shader_source_text('__proto__'), 'prototype shader');
  assert.equal(host.update('__proto__', 'updated prototype shader'), 2);
});
