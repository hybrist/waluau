import assert from 'node:assert/strict';
import { test } from 'node:test';

import { buildWaluauImports, luauToString, WALUAU_IMPORT_MODULE } from './runtime.js';

test('projects buffer strings one byte per browser string code unit', () => {
  const names = ['buffer_string_len', 'buffer_string_read', 'buffer_string_write'];
  const runtime = buildWaluauImports(null, undefined, {
    requiredImports: [
      ...names.map(name => ({ module: WALUAU_IMPORT_MODULE, name, kind: 'function' })),
      { module: WALUAU_IMPORT_MODULE, name: 'memory', kind: 'memory' },
    ],
    bytesConstants: [],
  })[WALUAU_IMPORT_MODULE];
  const allBytes = Array.from({ length: 256 }, (_, byte) => String.fromCharCode(byte)).join('');

  assert.equal(runtime.buffer_string_len(allBytes), 256);
  assert.equal(runtime.buffer_string_len('a\u0100'), -1);
  runtime.buffer_string_write(allBytes, 32, allBytes.length);
  assert.equal(runtime.buffer_string_read(32, allBytes.length), allBytes);
  runtime.buffer_string_write('a\0\xff', 512, 3);
  assert.deepEqual(Array.from(new Uint8Array(runtime.memory.buffer, 512, 3)), [97, 0, 255]);
});

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

test('stringifies numbers with the shortest round-tripping decimal', () => {
  // Golden values from the upstream strconv conformance chunks, which pin
  // Luau's `luai_num2str` output. Each needs a different digit count, so a
  // fixed-precision conversion cannot satisfy them all.
  assert.equal(luauToString(Math.PI), '3.141592653589793');
  assert.equal(luauToString(2.0049288280105384), '2.0049288280105384');
  assert.equal(luauToString(-0.00610404721867928), '-0.00610404721867928');
  assert.equal(luauToString(1.3202313930270133e-192), '1.3202313930270133e-192');
  assert.equal(luauToString(1.1295093211933533e65), '1.1295093211933533e+65');
  // Shorter than its literal because the literal is not the shortest decimal
  // that selects this double.
  assert.equal(luauToString(2.0563000527063302), '2.05630005270633');
  assert.equal(luauToString(4.8970527433648997e-260), '4.8970527433649e-260');
  assert.equal(luauToString(-1.9490628022799998e289), '-1.94906280228e+289');
});

test('stringifies integers exactly across the f64 integral range', () => {
  assert.equal(luauToString(0), '0');
  assert.equal(luauToString(-0), '-0');
  assert.equal(luauToString(5), '5');
  // 2^53 - 1 and a neighbour: truncating to 14 significant digits would round
  // these to trailing zeros.
  assert.equal(luauToString(9007199254740991), '9007199254740991');
  assert.equal(luauToString(1125968630513728), '1125968630513728');
  assert.equal(luauToString(9007199254740992), '9007199254740992');
  // Above 2^53 the shortest decimal is padded out with zeros rather than
  // pretending to more precision than the double carries.
  assert.equal(luauToString(3.6984408976312836e19), '36984408976312840000');
  assert.equal(luauToString(1e21), '1e+21');
  assert.equal(luauToString(-1e24), '-1e+24');
});

test('switches to scientific notation on Luau boundaries with a padded exponent', () => {
  // Fixed point holds while the decimal point sits in [-5, 21].
  assert.equal(luauToString(3.0517578125e-5), '0.000030517578125');
  assert.equal(luauToString(1e-6), '0.000001');
  assert.equal(luauToString(1e20), '100000000000000000000');
  assert.equal(luauToString(1e-7), '1e-07');
  assert.equal(luauToString(1.5e-7), '1.5e-07');
  assert.equal(luauToString(5e-324), '5e-324');
  assert.equal(luauToString(1.7976931348623157e308), '1.7976931348623157e+308');
});

test('stringifies numeric specials with Luau spellings', () => {
  assert.equal(luauToString(NaN), 'nan');
  assert.equal(luauToString(Infinity), 'inf');
  assert.equal(luauToString(-Infinity), '-inf');
});

test('keeps string.format %g on a fixed precision rather than tostring', () => {
  const format = buildWaluauImports(null, undefined, {
    requiredImports: [
      { module: WALUAU_IMPORT_MODULE, name: 'string_format1', kind: 'function' },
    ],
    bytesConstants: [],
  })[WALUAU_IMPORT_MODULE].string_format1;

  assert.equal(format('%g', 12.5), '12.5');
  // Unlike `tostring`, a bare `%g` does not widen to the shortest round-trip;
  // see waluau-zbiu for narrowing it further to C's default precision of 6.
  assert.equal(format('%g', Math.PI), '3.1415926535898');
  assert.equal(format('%.3g', Math.PI), '3.14');
});

test('implements Luau scalar math edge semantics', () => {
  const names = [
    'math.min', 'math.max', 'math.modf', 'math.frexp', 'math.ldexp',
    'math.log', 'math.sign', 'math.clamp', 'math.round', 'math.lerp',
    'math.isnan', 'math.isinf', 'math.isfinite', 'math.noise',
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
  // Golden values cross-checked against Luau 86d2a9d with
  // FFlagFixMathNoisePrecision enabled.
  assert.equal(math['math.noise'](0.5), 0);
  assert.equal(math['math.noise'](0.5, 0.5), -0.25);
  assert.equal(math['math.noise'](0.5, 0.5, -0.5), 0.125);
  assert.equal(
    math['math.noise'](455.7204209769105, 340.80410508750134, 121.80087666537628),
    0.5010709762573242,
  );
  assert.equal(math['math.noise'](-1.25, 2.75, -3.5), 0.40050268173217773);
  assert.equal(math['math.noise'](-1.25 + 256, 2.75 - 512, -3.5 + 768), 0.40050268173217773);
  assert.equal(math['math.noise'](2 ** 40 + 0.25), 0.146484375);
  assert.equal(math['math.noise'](-(2 ** 40) - 0.25), -0.3017578125);
  assert.ok(Number.isNaN(math['math.noise'](NaN)));
  assert.ok(Number.isNaN(math['math.noise'](Infinity)));
});
