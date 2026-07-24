// Generates assets/ice-noise.png, the tileable 256x256 RGBA data texture the
// ice-freeze card shader samples. Each channel is an independent ingredient:
//   R  multi-octave value noise      - frost grain and creep-front distortion
//   G  voronoi F2-F1 ridge field     - ice chunk borders and crack lines
//   B  per-voronoi-cell random value - flat facet shading aligned with G
//   A  high-frequency value noise    - glints and sparkle
// Every lattice wraps at the image border, so the shader may tile it freely.
//
// Run from apps/ante:  node tools/generate-ice-noise.mjs

import { deflateSync } from 'node:zlib';
import { writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const SIZE = 256;

// Deterministic integer hash so the asset is reproducible byte-for-byte.
function hash2(x, y, seed) {
  let h = (x * 374761393 + y * 668265263 + seed * 2147483647) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  h ^= h >>> 16;
  return (h >>> 0) / 4294967295;
}

function smootherstep(t) {
  return t * t * t * (t * (t * 6 - 15) + 10);
}

// Value noise on a lattice of `cells` points that wraps at the image edge.
function valueNoise(u, v, cells, seed) {
  const x = u * cells;
  const y = v * cells;
  const x0 = Math.floor(x);
  const y0 = Math.floor(y);
  const fx = smootherstep(x - x0);
  const fy = smootherstep(y - y0);
  const wrap = (n) => ((n % cells) + cells) % cells;
  const c00 = hash2(wrap(x0), wrap(y0), seed);
  const c10 = hash2(wrap(x0 + 1), wrap(y0), seed);
  const c01 = hash2(wrap(x0), wrap(y0 + 1), seed);
  const c11 = hash2(wrap(x0 + 1), wrap(y0 + 1), seed);
  const top = c00 + (c10 - c00) * fx;
  const bottom = c01 + (c11 - c01) * fx;
  return top + (bottom - top) * fy;
}

function fbm(u, v, baseCells, octaves, seed) {
  let total = 0;
  let amplitude = 1;
  let scale = 0;
  for (let octave = 0; octave < octaves; octave += 1) {
    total += valueNoise(u, v, baseCells << octave, seed + octave * 101) * amplitude;
    scale += amplitude;
    amplitude *= 0.5;
  }
  return total / scale;
}

// Wrapped voronoi: nearest and second-nearest jittered feature points.
function voronoi(u, v, cells, seed) {
  const x = u * cells;
  const y = v * cells;
  const x0 = Math.floor(x);
  const y0 = Math.floor(y);
  const wrap = (n) => ((n % cells) + cells) % cells;
  let f1 = 1e9;
  let f2 = 1e9;
  let cellValue = 0;
  for (let dy = -1; dy <= 1; dy += 1) {
    for (let dx = -1; dx <= 1; dx += 1) {
      const cx = x0 + dx;
      const cy = y0 + dy;
      const jx = hash2(wrap(cx), wrap(cy), seed);
      const jy = hash2(wrap(cx), wrap(cy), seed + 7919);
      const px = cx + jx;
      const py = cy + jy;
      const distance = Math.hypot(px - x, py - y);
      if (distance < f1) {
        f2 = f1;
        f1 = distance;
        cellValue = hash2(wrap(cx), wrap(cy), seed + 104729);
      } else if (distance < f2) {
        f2 = distance;
      }
    }
  }
  return { f1, f2, cellValue };
}

const pixels = Buffer.alloc(SIZE * SIZE * 4);
for (let py = 0; py < SIZE; py += 1) {
  for (let px = 0; px < SIZE; px += 1) {
    const u = px / SIZE;
    const v = py / SIZE;
    const grain = fbm(u, v, 8, 5, 1);
    const { f1, f2, cellValue } = voronoi(u, v, 8, 2);
    const ridge = Math.min((f2 - f1) * 1.9, 1);
    const sparkle = fbm(u, v, 64, 2, 3);
    const offset = (py * SIZE + px) * 4;
    pixels[offset] = Math.round(grain * 255);
    pixels[offset + 1] = Math.round(ridge * 255);
    pixels[offset + 2] = Math.round(cellValue * 255);
    pixels[offset + 3] = Math.round(sparkle * 255);
  }
}

const CRC_TABLE = new Int32Array(256);
for (let n = 0; n < 256; n += 1) {
  let c = n;
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  CRC_TABLE[n] = c;
}

function crc32(buffer) {
  let c = 0xffffffff;
  for (const byte of buffer) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'latin1'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

const header = Buffer.alloc(13);
header.writeUInt32BE(SIZE, 0);
header.writeUInt32BE(SIZE, 4);
header[8] = 8; // bit depth
header[9] = 6; // color type RGBA
const scanlines = Buffer.alloc(SIZE * (SIZE * 4 + 1));
for (let py = 0; py < SIZE; py += 1) {
  const row = py * (SIZE * 4 + 1);
  scanlines[row] = 0; // filter: none
  pixels.copy(scanlines, row + 1, py * SIZE * 4, (py + 1) * SIZE * 4);
}
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', header),
  chunk('IDAT', deflateSync(scanlines, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
]);

const output = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'assets', 'ice-noise.png');
writeFileSync(output, png);
console.log(`wrote ${join('assets', 'ice-noise.png')} (${png.length} bytes)`);
