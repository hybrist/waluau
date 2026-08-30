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
