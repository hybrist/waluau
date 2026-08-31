import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = dirname(fileURLToPath(import.meta.url));
const expectedPendingByFamily = {
  apicalls: 1,
  assert: 1,
  attrib: 2,
  basic: 14,
  bitwise: 12,
  buffers: 2,
  calls: 43,
  classes: 49,
  clear: 1,
  closure: 19,
  constructs: 41,
  coroutine: 22,
  coverage: 1,
  cyield: 13,
  datetime: 1,
  debug: 1,
  debugger: 1,
  errors: 80,
  events: 25,
  exceptions: 1,
  explicit_type_instantiations: 1,
  gc: 22,
  ifelseexpr: 3,
  integers: 19,
  integers_regspill: 6,
  interrupt: 1,
  iter: 31,
  iter_fenv: 1,
  literals: 4,
  locals: 1,
  math: 7,
  move: 1,
  native: 51,
  native_integer_spills: 3,
  native_types: 1,
  native_userdata: 1,
  ndebug_upvalues: 1,
  pcall: 66,
  pm: 52,
  safeenv: 1,
  sort: 1,
  stringinterp: 4,
  strings: 25,
  tables: 4,
  tmerror: 1,
  tpack: 19,
  types: 1,
  udata_direct: 1,
  userdata: 1,
  utf8: 1,
  vararg: 1,
  vector: 1,
  vector_library: 11,
};

// Every pending chunk inherits one of these exact family mappings. `deviation:`
// entries name sections in DEVIATIONS.md; `bead:` entries name open tracked
// implementation work. A family may have both because its chunks can contain
// several independent blockers.
const trackedByFamily = {
  apicalls: 'deviation:embedding-hooks',
  assert: 'deviation:strict-bool; bead:waluau-9f8d',
  attrib: 'deviation:sparse-mixed-hash-tables',
  basic: 'deviation:sparse-mixed-hash-tables,strict-bool,aot-loadstring,static-names; bead:waluau-9f8d',
  bitwise: 'bead:waluau-rndq,waluau-dbyy',
  buffers: 'deviation:embedding-hooks; bead:waluau-2dow',
  calls: 'deviation:aot-loadstring,sparse-mixed-hash-tables; bead:waluau-jnyd,waluau-zxju,waluau-n6u8,waluau-9f8d',
  classes: 'bead:waluau-wll8',
  clear: 'deviation:sparse-mixed-hash-tables',
  closure: 'deviation:typed-coroutine,aot-loadstring,sparse-mixed-hash-tables,static-names; bead:waluau-9f8d',
  constructs: 'deviation:strict-bool,aot-loadstring,sparse-mixed-hash-tables; bead:waluau-9f8d',
  coroutine: 'deviation:typed-coroutine',
  coverage: 'deviation:embedding-hooks',
  cyield: 'deviation:native-c-yield,typed-coroutine',
  datetime: 'bead:waluau-qabb',
  debug: 'deviation:embedding-hooks',
  debugger: 'deviation:embedding-hooks',
  errors: 'deviation:aot-loadstring,sparse-mixed-hash-tables; bead:waluau-wb7a,waluau-fg46,waluau-9f8d',
  events: 'deviation:metatable-events',
  exceptions: 'deviation:embedding-hooks',
  explicit_type_instantiations: 'bead:waluau-9ttd',
  gc: 'deviation:wasm-gc-observability',
  ifelseexpr: 'deviation:strict-bool',
  integers: 'deviation:luau-integer-vm-extension',
  integers_regspill: 'deviation:native-jit-register-layout',
  interrupt: 'deviation:embedding-hooks',
  iter: 'deviation:sparse-mixed-hash-tables,typed-coroutine,metatable-events; bead:waluau-zxju,waluau-n6u8,waluau-yfus',
  iter_fenv: 'deviation:embedding-hooks',
  literals: 'deviation:aot-loadstring,sparse-mixed-hash-tables; bead:waluau-9f8d',
  locals: 'deviation:aot-loadstring,sparse-mixed-hash-tables',
  math: 'deviation:aot-loadstring; bead:waluau-2dow,waluau-8fxn,waluau-jnyd,waluau-n6u8',
  move: 'deviation:sparse-mixed-hash-tables',
  native: 'deviation:native-jit-register-layout',
  native_integer_spills: 'deviation:native-jit-register-layout',
  native_types: 'deviation:native-jit-register-layout',
  native_userdata: 'deviation:native-jit-register-layout,reference-test-userdata',
  ndebug_upvalues: 'deviation:embedding-hooks',
  pcall: 'deviation:typed-coroutine; bead:waluau-8fxn,waluau-wb7a,waluau-esz6,waluau-9f8d',
  pm: 'deviation:aot-loadstring,sparse-mixed-hash-tables; bead:waluau-dbyy,waluau-esz6,waluau-wb7a',
  safeenv: 'deviation:embedding-hooks',
  sort: 'deviation:aot-loadstring,sparse-mixed-hash-tables',
  stringinterp: 'deviation:static-names; bead:waluau-h37g',
  strings: 'deviation:aot-loadstring,sparse-mixed-hash-tables,metatable-events; bead:waluau-dbyy,waluau-nlyf,waluau-vogb,waluau-fg46,waluau-9f8d',
  tables: 'deviation:aot-loadstring,sparse-mixed-hash-tables; bead:waluau-zxju',
  tmerror: 'deviation:metatable-events',
  tpack: 'deviation:binary-packing',
  types: 'deviation:embedding-hooks',
  udata_direct: 'deviation:reference-test-userdata',
  userdata: 'deviation:reference-test-userdata',
  utf8: 'deviation:aot-loadstring; bead:waluau-zxju',
  vararg: 'deviation:sparse-mixed-hash-tables; bead:waluau-n6u8,waluau-zxju',
  vector: 'bead:waluau-uneu',
  vector_library: 'bead:waluau-uneu',
};

const documentedDeviations = new Set([
  'aot-loadstring',
  'binary-packing',
  'embedding-hooks',
  'luau-integer-vm-extension',
  'metatable-events',
  'native-c-yield',
  'native-jit-register-layout',
  'reference-test-userdata',
  'sparse-mixed-hash-tables',
  'static-names',
  'strict-bool',
  'typed-coroutine',
  'wasm-gc-observability',
]);
const openBeads = new Set([
  'waluau-2dow',
  'waluau-8fxn',
  'waluau-9f8d',
  'waluau-9ttd',
  'waluau-dbyy',
  'waluau-esz6',
  'waluau-fg46',
  'waluau-h37g',
  'waluau-jnyd',
  'waluau-n6u8',
  'waluau-nlyf',
  'waluau-qabb',
  'waluau-rndq',
  'waluau-uneu',
  'waluau-vogb',
  'waluau-wb7a',
  'waluau-wll8',
  'waluau-yfus',
  'waluau-zxju',
]);

const range = (stem, from, to) =>
  Array.from({ length: to - from + 1 }, (_, offset) => `${stem}.${from + offset}.walu`);
const names = (stem, numbers) => numbers.map((number) => `${stem}.${number}.walu`);

const intentionalExecutionModel = new Set([
  ...names('native', [
    ...Array.from({ length: 6 }, (_, index) => index + 2),
    ...Array.from({ length: 9 }, (_, index) => index + 10),
    ...Array.from({ length: 25 }, (_, index) => index + 21),
    47, 48, 49,
    ...Array.from({ length: 8 }, (_, index) => index + 51),
  ]),
  ...range('integers', 1, 19),
  ...range('integers_regspill', 1, 6),
  ...range('native_integer_spills', 1, 3),
  'native_types.walu',
  'native_userdata.walu',
  ...names('gc', [2, 3, 4, ...Array.from({ length: 19 }, (_, index) => index + 6)]),
  ...range('events', 1, 25),
  ...range('cyield', 1, 13),
  'apicalls.walu',
  'exceptions.walu',
  'coverage.walu',
  'debug.walu',
  'debugger.walu',
  'interrupt.walu',
  'iter_fenv.walu',
  'ndebug_upvalues.walu',
  'safeenv.walu',
  'types.walu',
  'udata_direct.walu',
  'userdata.walu',
]);

const files = (await readdir(directory)).filter((file) => file.endsWith('.walu'));
const sources = new Map(
  await Promise.all(files.map(async (file) => [file, await readFile(resolve(directory, file), 'utf8')])),
);
const pending = [...sources]
  .filter(([, source]) => /^-- conformance: pending$/m.test(source))
  .map(([file]) => file)
  .sort();
const actualPendingByFamily = Object.fromEntries(
  Object.keys(expectedPendingByFamily).map((family) => [family, 0]),
);
const deviationsDocument = await readFile(resolve(directory, 'DEVIATIONS.md'), 'utf8');
const familyTable = deviationsDocument
  .split('## Exhaustive pending-family mapping')[1]
  ?.split('## Fixable gaps remain tracked work')[0];
assert.ok(familyTable, 'DEVIATIONS.md is missing the exhaustive pending-family table');
const documentedFamilies = [];
let documentedPendingCount = 0;

for (const line of familyTable.split('\n').filter((candidate) => /^\| `/.test(candidate))) {
  const families = [...line.matchAll(/`([^`]+)\*`/g)].map((match) => match[1]);
  const reportedCount = Number(line.split('|')[2].trim());
  assert.ok(families.length > 0, `unparseable family table row: ${line}`);
  assert.equal(
    reportedCount,
    families.reduce((sum, family) => sum + (expectedPendingByFamily[family] ?? 0), 0),
    `family table row has a stale count: ${line}`,
  );
  documentedFamilies.push(...families);
  documentedPendingCount += reportedCount;
}

for (const file of pending) {
  const family = file.split('.')[0];
  assert.ok(
    Object.hasOwn(actualPendingByFamily, family),
    `pending chunk ${file} has no family mapping in DEVIATIONS.md`,
  );
  actualPendingByFamily[family] += 1;
}

assert.equal(files.length, 1090, 'total Luau chunk count changed; reconcile DEVIATIONS.md');
assert.equal(pending.length, 674, 'pending Luau chunk count changed; reconcile DEVIATIONS.md');
assert.equal(files.length - pending.length, 416, 'enabled Luau chunk count changed');
assert.deepEqual(actualPendingByFamily, expectedPendingByFamily, 'pending family counts changed');
assert.deepEqual(
  documentedFamilies.sort(),
  Object.keys(expectedPendingByFamily).sort(),
  'DEVIATIONS.md must name every pending family exactly once',
);
assert.equal(documentedPendingCount, pending.length, 'DEVIATIONS.md family counts do not sum to pending');
assert.deepEqual(
  Object.keys(trackedByFamily),
  Object.keys(expectedPendingByFamily),
  'every pending family must have a deviation or open-bead mapping',
);
for (const [family, mapping] of Object.entries(trackedByFamily)) {
  assert.match(mapping, /(?:deviation|bead):/, `${family} has no concrete inventory mapping`);
  for (const group of mapping.split('; ')) {
    const [kind, values] = group.split(':');
    const allowed = kind === 'deviation' ? documentedDeviations : kind === 'bead' ? openBeads : null;
    assert.ok(allowed, `${family} has unknown mapping kind ${kind}`);
    for (const value of values.split(',')) {
      assert.ok(allowed.has(value), `${family} maps to unknown ${kind} ${value}`);
    }
  }
}
assert.equal(intentionalExecutionModel.size, 153, 'intentional execution-model set is malformed');

for (const file of intentionalExecutionModel) {
  assert.ok(sources.has(file), `documented intentional chunk ${file} does not exist`);
  assert.match(sources.get(file), /^-- conformance: pending$/m, `${file} is no longer pending`);
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
  `Luau inventory verified: ${files.length} total, ${files.length - pending.length} enabled, ${pending.length} pending; ${intentionalExecutionModel.size} intentional execution-model chunks.`,
);
