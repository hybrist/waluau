import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import * as esbuild from 'esbuild';

const extensionRoot = path.dirname(fileURLToPath(import.meta.url));
const outputPath = path.join(extensionRoot, 'dist', 'extension.js');

export async function buildExtension({ write = true } = {}) {
  return esbuild.build({
    absWorkingDir: extensionRoot,
    bundle: true,
    entryPoints: [path.join(extensionRoot, 'extension.js')],
    external: ['vscode'],
    format: 'cjs',
    legalComments: 'none',
    metafile: true,
    outfile: outputPath,
    platform: 'node',
    write,
  });
}

async function main() {
  const check = process.argv.includes('--check');
  const result = await buildExtension({ write: !check });

  if (check) {
    assert.equal(result.outputFiles.length, 1);
    const committed = await readFile(outputPath);
    assert.ok(
      Buffer.compare(Buffer.from(result.outputFiles[0].contents), committed) === 0,
      'dist/extension.js is stale; run pnpm build in .vscode/extensions/waluau',
    );
    console.log('dist/extension.js matches the extension sources and lockfile');
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
