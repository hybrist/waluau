import assert from 'node:assert/strict';
import { readdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = dirname(fileURLToPath(import.meta.url));

// Whole-directory pins. Per-family numbers are NOT restated here: they are
// derived from the chunk markers below and written into DEVIATIONS.md by
// `--write`, because a hand-maintained per-family table merges silently between
// concurrent branches. These four totals still fail the gate on any drift.
const EXPECTED_TOTAL = 1098;
const EXPECTED_ENABLED = 441;
const EXPECTED_PENDING = 236;
const EXPECTED_UNTRIAGED = 30;
const EXPECTED_OUT_OF_SCOPE = 421;

// Deliberate differences between Waluau and the reference Luau implementation.
// A chunk blocked by one of these carries `-- conformance: out-of-scope: <slug>`
// and nobody is expected to make it pass. Every slug has a section in
// DEVIATIONS.md stating the difference and why Waluau keeps it.
const documentedDeviations = new Set([
  'aot-loadstring',
  'binary-packing',
  'browser-clocks-and-calendars',
  'embedding-hooks',
  'heterogeneous-values',
  'luau-integer-vm-extension',
  'metatable-events',
  'native-c-yield',
  'native-jit-register-layout',
  'reference-test-userdata',
  'reserved-type-keywords',
  'sparse-mixed-hash-tables',
  'static-names',
  'static-type-errors',
  'strict-bool',
  'typed-coroutine',
  'wasm-gc-observability',
]);

// Open tracked work covering the chunks that carry `-- conformance: pending`.
const openBeads = new Set([
  'waluau-274e',
  'waluau-2dow',
  'waluau-31kg',
  'waluau-3em1',
  'waluau-4487',
  'waluau-844l',
  'waluau-9f8d',
  'waluau-9ttd',
  'waluau-d480',
  'waluau-dbyy',
  'waluau-esz6',
  'waluau-j74d',
  'waluau-jehg',
  'waluau-lz2e',
  'waluau-n6u8',
  'waluau-nlyf',
  'waluau-nsp4',
  'waluau-pndm',
  'waluau-rndq',
  'waluau-uneu',
  'waluau-vogb',
  'waluau-wb7a',
  'waluau-wll8',
  'waluau-zxju',
]);

// Beads owning the remaining pending chunks of each family. Counts are derived,
// so this table only records attribution. A family listed here with no pending
// chunk left, or a pending family missing from it, fails the gate.
const trackedByFamily = {
  assert: 'waluau-9f8d',
  attrib: 'waluau-zxju',
  basic: 'waluau-jehg,waluau-pndm,waluau-n6u8,waluau-9f8d',
  bitwise: 'waluau-dbyy,waluau-esz6,waluau-rndq,waluau-3em1',
  buffers: 'waluau-2dow',
  calls: 'waluau-j74d,waluau-jehg,waluau-zxju,waluau-2dow,waluau-lz2e,waluau-9f8d',
  classes: 'waluau-wll8',
  closure: 'waluau-j74d,waluau-9f8d',
  constructs: 'waluau-jehg,waluau-9f8d',
  datetime: 'waluau-31kg,waluau-9f8d',
  errors: 'waluau-wb7a,waluau-jehg,waluau-844l',
  explicit_type_instantiations: 'waluau-9ttd',
  iter: 'waluau-j74d,waluau-dbyy,waluau-3em1,waluau-n6u8',
  math: 'waluau-dbyy,waluau-jehg,waluau-n6u8,waluau-9f8d',
  native:
    'waluau-j74d,waluau-pndm,waluau-31kg,waluau-9ttd,waluau-uneu,waluau-2dow,waluau-nsp4,waluau-rndq,waluau-esz6,waluau-9f8d',
  native_integer_spills: 'waluau-3em1',
  pcall: 'waluau-wb7a,waluau-zxju,waluau-jehg,waluau-n6u8,waluau-274e',
  pm: 'waluau-zxju,waluau-lz2e,waluau-dbyy,waluau-esz6,waluau-4487,waluau-274e,waluau-j74d',
  strings: 'waluau-j74d,waluau-esz6,waluau-nlyf,waluau-vogb,waluau-nsp4,waluau-dbyy,waluau-9f8d',
  tables: 'waluau-jehg,waluau-9f8d',
  vector: 'waluau-uneu',
  vector_library: 'waluau-uneu',
};

const PENDING_DIRECTIVE = /^-- conformance: pending$/m;
const UNTRIAGED_DIRECTIVE = /^-- conformance: untriaged: (.+)$/m;
const OUT_OF_SCOPE_DIRECTIVE = /^-- conformance: out-of-scope: (.+)$/m;

const files = (await readdir(directory)).filter((file) => file.endsWith('.walu'));
const sources = new Map(
  await Promise.all(
    files.map(async (file) => [file, await readFile(resolve(directory, file), 'utf8')]),
  ),
);

// `untriaged` is a variant of `pending`, not a third category: an untriaged
// chunk is counted in `pending` and reported as a subset of it. The three
// directives are mutually exclusive.
const pending = [];
const untriaged = new Map(); // file -> free-text reason
const outOfScope = new Map(); // file -> deviation slugs
for (const [file, source] of [...sources].sort(([a], [b]) => a.localeCompare(b))) {
  const isPending = PENDING_DIRECTIVE.test(source);
  const untriagedMatch = source.match(UNTRIAGED_DIRECTIVE);
  const outOfScopeMatch = source.match(OUT_OF_SCOPE_DIRECTIVE);
  const markers = [isPending, untriagedMatch, outOfScopeMatch].filter(Boolean).length;
  assert.ok(
    markers <= 1,
    `${file} carries more than one of 'pending', 'untriaged' and 'out-of-scope'; a chunk carries exactly one`,
  );
  if (untriagedMatch) {
    const reason = untriagedMatch[1].trim();
    assert.ok(
      reason.length >= 20,
      `${file} must say what the open question is after 'untriaged:', not just that there is one`,
    );
    untriaged.set(file, reason);
    pending.push(file);
    continue;
  }
  if (isPending) {
    pending.push(file);
    continue;
  }
  if (!outOfScopeMatch) continue;
  const slugs = outOfScopeMatch[1].split(',').map((slug) => slug.trim());
  for (const slug of slugs) {
    assert.ok(
      documentedDeviations.has(slug),
      `${file} names undocumented deviation '${slug}'; add a DEVIATIONS.md section first`,
    );
  }
  outOfScope.set(file, slugs);
}

const enabled = files.length - pending.length - outOfScope.size;
assert.equal(files.length, EXPECTED_TOTAL, 'total Luau chunk count changed; reconcile DEVIATIONS.md');
assert.equal(enabled, EXPECTED_ENABLED, 'enabled Luau chunk count changed; reconcile DEVIATIONS.md');
assert.equal(pending.length, EXPECTED_PENDING, 'pending chunk count changed; reconcile DEVIATIONS.md');
assert.equal(
  untriaged.size,
  EXPECTED_UNTRIAGED,
  'untriaged chunk count changed; reconcile DEVIATIONS.md',
);
assert.equal(
  outOfScope.size,
  EXPECTED_OUT_OF_SCOPE,
  'out-of-scope chunk count changed; reconcile DEVIATIONS.md',
);

const familyOf = (file) => file.replace(/\.walu$/, '').split('.')[0];

const pendingByFamily = new Map();
const untriagedByFamily = new Map();
for (const file of pending) {
  const family = familyOf(file);
  pendingByFamily.set(family, (pendingByFamily.get(family) ?? 0) + 1);
  if (untriaged.has(file)) {
    untriagedByFamily.set(family, (untriagedByFamily.get(family) ?? 0) + 1);
  }
}
const chunksByDeviation = new Map();
const familiesByDeviation = new Map();
for (const [file, slugs] of outOfScope) {
  for (const slug of slugs) {
    chunksByDeviation.set(slug, (chunksByDeviation.get(slug) ?? 0) + 1);
    if (!familiesByDeviation.has(slug)) familiesByDeviation.set(slug, new Set());
    familiesByDeviation.get(slug).add(familyOf(file));
  }
}

assert.deepEqual(
  Object.keys(trackedByFamily).sort(),
  [...pendingByFamily.keys()].sort(),
  'every pending family must name its open beads, and only pending families may appear',
);
for (const [family, beads] of Object.entries(trackedByFamily)) {
  for (const bead of beads.split(',')) {
    assert.ok(openBeads.has(bead), `${family} maps to unknown or closed bead ${bead}`);
  }
}
for (const slug of documentedDeviations) {
  assert.ok(chunksByDeviation.has(slug), `documented deviation '${slug}' has no chunk; drop it`);
}

// The two inventory tables in DEVIATIONS.md are generated from the markers
// above, so a stale count cannot survive a merge. `--write` regenerates them.
const deviationsPath = resolve(directory, 'DEVIATIONS.md');
const deviationsDocument = await readFile(deviationsPath, 'utf8');
const generated = {
  'out-of-scope': [
    '| Deviation | Out-of-scope chunks | Families |',
    '| --- | ---: | --- |',
    ...[...chunksByDeviation.entries()]
      .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
      .map(
        ([slug, count]) =>
          `| \`${slug}\` | ${count} | ${[...familiesByDeviation.get(slug)]
            .sort()
            .map((family) => `\`${family}\``)
            .join(', ')} |`,
      ),
    '',
    `A chunk may name more than one deviation, so these counts sum to more than the ${outOfScope.size} out-of-scope chunks. List the exact set for one deviation with \`rg -l 'out-of-scope:.*<slug>' conformance/luau\`.`,
  ].join('\n'),
  pending: [
    '| Pending family | Chunks | Untriaged | Open beads |',
    '| --- | ---: | ---: | --- |',
    ...[...pendingByFamily.entries()]
      .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
      .map(
        ([family, count]) =>
          `| \`${family}*\` | ${count} | ${untriagedByFamily.get(family) ?? 0} | ${trackedByFamily[
            family
          ]
            .split(',')
            .map((bead) => `\`${bead}\``)
            .join(', ')} |`,
      ),
    '',
    `Total: ${pending.length} pending chunks in ${pendingByFamily.size} families, of which ${untriaged.size} are untriaged.`,
  ].join('\n'),
  untriaged: [
    '| Chunk | Open question |',
    '| --- | --- |',
    ...[...untriaged.entries()]
      .sort(([a], [b]) => a.localeCompare(b, undefined, { numeric: true }))
      .map(([file, reason]) => `| \`${file.replace(/\.walu$/, '')}\` | ${reason} |`),
  ].join('\n'),
};

let updated = deviationsDocument;
for (const [key, body] of Object.entries(generated)) {
  const begin = `<!-- generated:${key} -->`;
  const end = `<!-- /generated:${key} -->`;
  const block = new RegExp(`${begin}[\\s\\S]*?${end}`);
  assert.match(deviationsDocument, block, `DEVIATIONS.md is missing the ${key} generated block`);
  updated = updated.replace(block, `${begin}\n\n${body}\n\n${end}`);
}

if (process.argv.includes('--write')) {
  await writeFile(deviationsPath, updated);
} else {
  assert.equal(
    updated,
    deviationsDocument,
    'DEVIATIONS.md inventory tables are stale; rerun this script with --write',
  );
}

const runner = await readFile(
  resolve(directory, '../../apps/conformance-runner/tests/conformance.test.js'),
  'utf8',
);
assert.match(
  runner,
  /const INTENTIONAL_VM_JIT_EXCLUSIONS = new Set\(\['luau\/native\.53\.walu'\]\);/,
  'native.53 must remain the sole exact-name browser-runner exclusion',
);

console.log(
  `Luau inventory verified: ${files.length} total, ${enabled} enabled, ` +
    `${pending.length} pending (${untriaged.size} untriaged), ` +
    `${outOfScope.size} out of scope across ${chunksByDeviation.size} documented deviations.`,
);
