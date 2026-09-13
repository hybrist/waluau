import { readdir, readFile } from 'node:fs/promises';
import { resolve, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const testsRoot = fileURLToPath(new URL('../tests/', import.meta.url));

// Deliberately limited to gameplay sources. Renderer probes and Storybook
// snapshots have their own directories and commands.
const rules = [
  ['pixel or screenshot observation', /\b(?:readPixels|getImageData|toDataURL|toBlob|screenshot|toHaveScreenshot|toMatchSnapshot)\s*\(/g],
  ['computed pixel or screenshot observation', /\[\s*['"](?:readPixels|getImageData|toDataURL|toBlob|screenshot|toHaveScreenshot|toMatchSnapshot)['"]\s*\]\s*\(/g],
  ['fixed timing wait', /\b(?:waitForTimeout|setTimeout|sleep)\s*\(/g],
  ['obsolete visual readiness helper', /\b(?:frameSignature|count\w*Ink|settleBoard)\b/g],
  ['visual or renderer dependency', /\b(?:from\s*|import\s*\()\s*['"][^'"]*(?:\/renderer\/|\/visual\/)[^'"]*['"]/g],
];

function withoutComments(source) {
  // Preserve offsets/newlines for diagnostics and do not treat URL strings
  // as comments. String contents stay visible for computed member checks.
  return source.replace(/(['"`])(?:\\[\s\S]|(?!\1)[^\\])*?\1|\/\/[^\n]*|\/\*[\s\S]*?\*\//g,
    (token) => token.startsWith('//') || token.startsWith('/*')
      ? token.replace(/[^\n]/g, ' ') : token);
}

export function checkGameplaySource(source, filename) {
  const code = withoutComments(source);
  const failures = [];
  for (const [reason, pattern] of rules) {
    for (const match of code.matchAll(pattern)) {
      const line = code.slice(0, match.index).split('\n').length;
      failures.push(`${filename}:${line}: ${reason}: ${match[0]}`);
    }
  }
  return failures;
}

export async function checkGameplayTree(root = testsRoot) {
  const failures = [];
  async function visit(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) await visit(path);
      else if (/\.[cm]?js$/.test(entry.name)) {
        failures.push(...checkGameplaySource(await readFile(path, 'utf8'), relative(root, path)));
      }
    }
  }
  await visit(root);
  return failures;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const failures = await checkGameplayTree();
  if (failures.length) {
    console.error(failures.join('\n'));
    process.exitCode = 1;
  } else console.log('Gameplay sources use semantic observations and readiness.');
}
