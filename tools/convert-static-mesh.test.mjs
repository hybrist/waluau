import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { convertGlb } from './convert-static-mesh.mjs';
const root = new URL('../apps/ante/assets/church/', import.meta.url);

test('church LoDs preserve height while reducing actual indexed GPU budgets', () => {
  let previous = Infinity;
  for (let level = 0; level < 4; level++) {
    const { vertices, indices, stats } = convertGlb(readFileSync(new URL(`church-lod${level}.glb`, root)));
    assert(stats.gpuVertices < previous);
    previous = stats.gpuVertices;
    assert(Math.abs(stats.maximum[1] - stats.minimum[1] - 17.45) < .05);
    assert(indices.every(i => i >= 0 && i < vertices.length / 9));
    assert(vertices.every(Number.isFinite));
    // RGB remains attached to each indexed vertex after primitive merging.
    assert(new Set(Array.from({ length: vertices.length / 9 }, (_, i) => vertices.slice(i * 9 + 6, i * 9 + 9).join(','))).size >= 4);
  }
});

test('rejects malformed files and unintended extra exported scene meshes', () => {
  const source = readFileSync(new URL('church-lod0.glb', root));
  assert.throws(() => convertGlb(source.subarray(0, source.length - 4)), /invalid GLB/);
  const jsonSize = source.readUInt32LE(12);
  const doc = JSON.parse(source.subarray(20, 20 + jsonSize).toString());
  doc.meshes.push(doc.meshes[0]);
  let json = Buffer.from(JSON.stringify(doc));
  json = Buffer.concat([json, Buffer.alloc((4 - json.length % 4) % 4, 32)]);
  const body = source.subarray(20 + jsonSize);
  const header = Buffer.alloc(20);
  source.copy(header, 0, 0, 20);
  header.writeUInt32LE(20 + json.length + body.length, 8);
  header.writeUInt32LE(json.length, 12);
  assert.throws(() => convertGlb(Buffer.concat([header, json, body])), /exactly one mesh/);
});
