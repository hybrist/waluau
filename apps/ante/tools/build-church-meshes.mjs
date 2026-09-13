import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { readFile, writeFile } from 'node:fs/promises';
import { convertGlb, waluauMesh } from '../../../tools/convert-static-mesh.mjs';
const assets = new URL('../assets/church/', import.meta.url);
const source = new URL('../src/church/', import.meta.url);
const stats = [];
for (let level = 0; level < 4; level++) {
  const mesh = convertGlb(await readFile(new URL(`church-lod${level}.glb`, assets)));
  await writeFile(new URL(`lod${level}.walu`, source), waluauMesh(mesh));
  stats.push({ level, ...mesh.stats });
}
await writeFile(new URL('gpu-lod-stats.json', assets), JSON.stringify(stats, null, 2) + '\n');
console.table(stats.map(({ level, gpuVertices, triangles, gpuBytes }) => ({ level, gpuVertices, triangles, gpuBytes })));

const formatted = spawnSync('cargo', ['run', '--quiet', '--release', '-p', 'waluau-cli', '--', 'fmt', ...stats.map(({level}) => fileURLToPath(new URL(`lod${level}.walu`, source)))], { cwd: new URL('../../..', import.meta.url), stdio: 'inherit' });
if (formatted.error) throw formatted.error;
if (formatted.status !== 0) throw new Error('Could not format generated church mesh modules');
