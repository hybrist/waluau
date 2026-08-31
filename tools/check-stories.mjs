// Compile every Storybook story in the repository.
//
// Stories are the one kind of Waluau source nothing else reaches. `./check`
// builds and tests the compiler, the Ante suites exercise the game's own
// modules, and `project.js` deliberately globs `*.stories.walu` out of the
// production module set — so a type error that only a story can reach lands on
// main and is first seen by a Vercel deploy running `pnpm build-storybook`.
// That is exactly how waluau-lcu9 happened: a change to a shared type left the
// stories behind, and every deploy failed for two weeks.
//
// This is the early half of the gate. It type-checks each story on its own,
// which is what catches that class of failure, and takes about as long as one
// cargo check. The other half already exists and is not repeated in CI: the
// Vercel deployment check runs the real `build-storybook` on every pull
// request, so a story broken by configuration this pass never loads — the
// story glob, the framework options, the shader source list — still cannot
// reach main. What that check cannot do is fail before a push, or say which
// story and which line rather than printing a deploy log.
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readdirSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const skipped = new Set(['node_modules', 'target', 'dist', '.git', 'storybook-static']);

function findStories(directory) {
  const found = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (skipped.has(entry.name)) continue;
      found.push(...findStories(join(directory, entry.name)));
    } else if (entry.name.endsWith('.stories.walu')) {
      found.push(join(directory, entry.name));
    }
  }
  return found;
}

// The asset manifest a story is compiled against is its app's, so resource
// types resolve the way the real Storybook build resolves them. Walk up from
// the story rather than hard-coding Ante's, so a second app is covered the day
// it gains a story.
function manifestFor(story) {
  let directory = dirname(story);
  while (directory.startsWith(root)) {
    const candidate = join(directory, 'waluau.assets.json');
    if (existsSync(candidate)) return candidate;
    if (directory === root) break;
    directory = dirname(directory);
  }
  return null;
}

const stories = findStories(root).sort();
if (stories.length === 0) {
  throw new Error('Story check found no *.stories.walu files; the discovery walk is wrong.');
}

const output = mkdtempSync(join(tmpdir(), 'waluau-stories-'));
const failures = [];
try {
  for (const story of stories) {
    const name = relative(root, story);
    const args = [
      'run', '--quiet', '-p', 'waluau-cli', '--',
      story,
      '-o', join(output, `${name.split(sep).join('-')}.wasm`),
      // --manifest is only accepted alongside --emit-js, and the JS shim is
      // what Storybook loads anyway.
      '--emit-js',
    ];
    const manifest = manifestFor(story);
    if (manifest) args.push('--manifest', manifest);
    try {
      execFileSync('cargo', args, { cwd: root, stdio: 'pipe' });
      console.log(`  ok  ${name}`);
    } catch (error) {
      const detail = `${error.stdout ?? ''}${error.stderr ?? ''}`.trim();
      failures.push(`${name}\n${detail}`);
      console.log(` FAIL ${name}`);
    }
  }
} finally {
  rmSync(output, { recursive: true, force: true });
}

if (failures.length > 0) {
  throw new Error(`Story compilation failed:\n\n${failures.join('\n\n')}`);
}
console.log(`Story check passed (${stories.length} stories).`);
