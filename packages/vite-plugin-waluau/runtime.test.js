import assert from 'node:assert/strict';
import { test } from 'node:test';

import { buildWaluauImports, WALUAU_IMPORT_MODULE } from './runtime.js';

test('builds and validates typed JSON through compiler host primitives', () => {
  const names = [
    '__json_pack_reset', '__json_pack_object', '__json_pack_object_set',
    '__json_pack_string', '__json_pack_i32', '__json_pack_finish',
    '__json_unpack_start', '__json_unpack_object_get', '__json_unpack_string',
    '__json_unpack_i32',
  ];
  const json = buildWaluauImports(null, undefined, {
    requiredImports: names.map(name => ({
      module: WALUAU_IMPORT_MODULE, name, kind: 'function',
    })),
    bytesConstants: [],
  })[WALUAU_IMPORT_MODULE];

  json.__json_pack_reset();
  const object = json.__json_pack_object();
  json.__json_pack_object_set(object, 'name', json.__json_pack_string('Ada "Lovelace"'));
  json.__json_pack_object_set(object, 'score', json.__json_pack_i32(42));
  const packed = json.__json_pack_finish(object);
  assert.equal(packed, '{"name":"Ada \\"Lovelace\\"","score":42}');

  const schema = JSON.stringify({
    t: 'record',
    f: [
      ['name', { t: 'string' }],
      ['score', { t: 'i32' }],
    ],
  });
  assert.equal(json.__json_unpack_start(packed, schema), '');
  assert.equal(json.__json_unpack_string(json.__json_unpack_object_get(0, 'name')), 'Ada "Lovelace"');
  assert.equal(json.__json_unpack_i32(json.__json_unpack_object_get(0, 'score')), 42);
  assert.match(json.__json_unpack_start('{"name":9,"score":42}', schema), /\$\.name: expected string/);
  assert.match(json.__json_unpack_start('{', schema), /^invalid JSON:/);
});

test('formats large finite numbers as fixed-point decimals', () => {
  const format = buildWaluauImports(null, undefined, {
    requiredImports: [
      { module: WALUAU_IMPORT_MODULE, name: 'string_format1', kind: 'function' },
    ],
    bytesConstants: [],
  })[WALUAU_IMPORT_MODULE].string_format1;

  assert.equal(format('%.0f', 1e21), '1000000000000000000000');
  assert.equal(format('%.6f', 1e21), '1000000000000000000000.000000');

  const exactNegative = BigInt(-1e308).toString();
  assert.equal(format('%.99f', -1e308), `${exactNegative}.${'0'.repeat(99)}`);
  assert.equal(format('%30.2f', -1e21), '    -1000000000000000000000.00');
  assert.equal(format('%-30.2f', 1e21), '1000000000000000000000.00     ');
});

test('fixed-point formatting preserves ordinary values, specials, and errors', () => {
  const format = buildWaluauImports(null, undefined, {
    requiredImports: [
      { module: WALUAU_IMPORT_MODULE, name: 'string_format1', kind: 'function' },
    ],
    bytesConstants: [],
  })[WALUAU_IMPORT_MODULE].string_format1;

  assert.equal(format('%f', 1.5), '1.500000');
  assert.equal(format('%.2f', -0), '0.00');
  assert.equal(format('%f', NaN), 'NaN');
  assert.equal(format('%f', Infinity), 'Infinity');
  assert.equal(format('%f', -Infinity), '-Infinity');
  assert.throws(() => format('%.123f', 1), /invalid string\.format precision/);
  assert.throws(() => format('%?', 1), /unsupported string\.format specifier/);
});

test('implements Luau scalar math edge semantics', () => {
  const names = [
    'math.min', 'math.max', 'math.modf', 'math.frexp', 'math.ldexp',
    'math.log', 'math.sign', 'math.clamp', 'math.round', 'math.lerp',
    'math.isnan', 'math.isinf', 'math.isfinite',
  ];
  const math = buildWaluauImports(null, undefined, {
    requiredImports: names.map(name => ({
      module: WALUAU_IMPORT_MODULE, name, kind: 'function',
    })),
    bytesConstants: [],
  })[WALUAU_IMPORT_MODULE];

  assert.ok(Number.isNaN(math['math.min'](NaN, 2)));
  assert.equal(math['math.min'](1, NaN), 1);
  assert.ok(Number.isNaN(math['math.max'](NaN, 2)));
  assert.equal(math['math.max'](1, NaN), 1);
  assert.deepEqual(math['math.modf'](3.5), [3, 0.5]);
  assert.deepEqual(math['math.modf'](-3), [-3, -0]);
  assert.deepEqual(math['math.modf'](-Infinity), [-Infinity, -0]);
  assert.deepEqual(math['math.frexp'](Math.PI), [Math.PI / 4, 2]);
  assert.deepEqual(math['math.frexp'](Number.MAX_VALUE), [Number.MAX_VALUE / (2 ** 1023) / 2, 1024]);
  assert.equal(math['math.ldexp'](Math.PI / 4, 2), Math.PI);
  assert.equal(math['math.ldexp'](0.5, 1024), 2 ** 1023);
  assert.equal(math['math.log'](8, 2), 3);
  assert.equal(math['math.sign'](NaN), 0);
  assert.equal(math['math.round'](0.5), 1);
  assert.equal(math['math.round'](-0.5), -1);
  assert.equal(math['math.round'](0.49999999999999994), 0);
  assert.equal(math['math.round'](-0.49999999999999994), -0);
  assert.equal(math['math.round'](Infinity), Infinity);
  assert.equal(math['math.lerp'](-Math.sqrt(3), Math.sqrt(2), 1), Math.sqrt(2));
  assert.equal(math['math.clamp'](4, 2, 3), 3);
  assert.throws(() => math['math.clamp'](1, 3, 2), /max must be greater/);
  assert.equal(math['math.isnan'](NaN), true);
  assert.equal(math['math.isinf'](-Infinity), true);
  assert.equal(math['math.isfinite'](123.45), true);
});
