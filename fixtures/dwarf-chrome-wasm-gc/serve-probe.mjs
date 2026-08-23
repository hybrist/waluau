import { createReadStream, statSync } from 'node:fs';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const fixtureDir = fileURLToPath(new URL('.', import.meta.url));
const extensionDir = process.argv[2];
const port = Number(process.env.PORT ?? 8123);

const roots = new Map([['/fixture/', fixtureDir]]);
if (extensionDir) roots.set('/extension/', extensionDir);
const types = new Map([
  ['.html', 'text/html; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.mjs', 'text/javascript; charset=utf-8'],
  ['.wasm', 'application/wasm'],
  ['.walu', 'text/plain; charset=utf-8'],
]);

createServer((request, response) => {
  const url = new URL(request.url, `http://${request.headers.host}`);
  if (url.pathname === '/') {
    response.writeHead(302, { location: '/fixture/probe.html' });
    response.end();
    return;
  }

  // Serve the harness from the extension path because WorkerPlugin constructs
  // its worker URL relative to the inspected document rather than import.meta.
  if (extensionDir && url.pathname === '/extension/extension-harness.html') {
    response.writeHead(200, {
      'content-type': 'text/html; charset=utf-8',
      'cross-origin-embedder-policy': 'require-corp',
      'cross-origin-opener-policy': 'same-origin',
    });
    createReadStream(join(fixtureDir, 'extension-harness.html')).pipe(response);
    return;
  }

  const match = [...roots].find(([prefix]) => url.pathname.startsWith(prefix));
  if (!match) {
    response.writeHead(404).end();
    return;
  }
  const [prefix, root] = match;
  const relative = normalize(url.pathname.slice(prefix.length)).replace(/^(\.\.[/\\])+/, '');
  const path = join(root, relative);
  try {
    if (!statSync(path).isFile()) throw new Error('not a file');
  } catch {
    response.writeHead(404).end();
    return;
  }
  response.writeHead(200, {
    'content-type': types.get(extname(path)) ?? 'application/octet-stream',
    'cross-origin-embedder-policy': 'require-corp',
    'cross-origin-opener-policy': 'same-origin',
  });
  createReadStream(path).pipe(response);
}).listen(port, '127.0.0.1', () => {
  process.stdout.write(`runtime: http://127.0.0.1:${port}/fixture/probe.html\n`);
  if (extensionDir) {
    process.stdout.write(`extension parser: http://127.0.0.1:${port}/extension/extension-harness.html\n`);
  }
});
