import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { extname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const app = fileURLToPath(new URL('..', import.meta.url));
const source = join(app, 'src');
const removed = [
  join(source, 'entity.walu'),
  join(source, 'box_layout.walu'),
  join(source, 'entities', 'adapter.walu'),
];
const forbidden = [
  /require\(["'][^"']*box_layout["']\)/,
  /require\(["'][^"']*entities\/adapter["']\)/,
  /require\(["'](?:\.\.\/)*entity["']\)/,
  /\bLegacy(?:Node|Style)\b/,
];

const failures = removed.filter(existsSync).map(path =>
  `removed compatibility file returned: ${relative(app, path)}`);

const visit = directory => {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) visit(path);
    else if (extname(path) === '.walu') {
      const text = readFileSync(path, 'utf8');
      for (const pattern of forbidden) {
        if (pattern.test(text)) {
          failures.push(`legacy retained-UI dependency in ${relative(app, path)}`);
          break;
        }
      }
    }
  }
};

visit(source);
if (failures.length > 0) {
  throw new Error(`Retained UI source check failed:\n${failures.join('\n')}`);
}

console.log('Retained UI source check passed.');
