import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const fixtureDir = dirname(fileURLToPath(import.meta.url));
const output = resolve(process.argv[2] ?? join(fixtureDir, 'dwarf_chrome_probe.wasm'));
const clang = process.env.WALUAU_DWARF_CLANG ?? 'clang';
const wasmLd = process.env.WALUAU_DWARF_WASM_LD ?? 'wasm-ld';
const wasmTools = process.env.WALUAU_DWARF_WASM_TOOLS ?? 'wasm-tools';
const dwarfVersion = process.env.WALUAU_DWARF_VERSION ?? '4';
if (!['4', '5'].includes(dwarfVersion)) {
  throw new Error('WALUAU_DWARF_VERSION must be 4 or 5');
}
const scratch = mkdtempSync(join(tmpdir(), 'waluau-dwarf-chrome-'));

function decodeUleb(bytes, offset) {
  let value = 0;
  let shift = 0;
  let byte;
  do {
    byte = bytes[offset++];
    value |= (byte & 0x7f) << shift;
    shift += 7;
  } while (byte & 0x80);
  return { value, offset };
}

function encodeUleb(value) {
  const bytes = [];
  do {
    let byte = value & 0x7f;
    value >>>= 7;
    if (value) byte |= 0x80;
    bytes.push(byte);
  } while (value);
  return Buffer.from(bytes);
}

function appendVectorEntry(payload, entry) {
  const count = decodeUleb(payload, 0);
  return Buffer.concat([
    encodeUleb(count.value + 1),
    payload.subarray(count.offset),
    Buffer.from(entry),
  ]);
}

function appendTargetFeature(payload, feature) {
  const nameLength = decodeUleb(payload, 0);
  const featuresStart = nameLength.offset + nameLength.value;
  const features = payload.subarray(featuresStart);
  return Buffer.concat([
    payload.subarray(0, featuresStart),
    appendVectorEntry(features, [0x2b, ...encodeUleb(feature.length), ...feature]),
  ]);
}

function addGcProbe(carrierWasm) {
  const chunks = [carrierWasm.subarray(0, 8)];
  let offset = 8;
  const exportName = Buffer.from('__synthetic_gc_round_trip');
  while (offset < carrierWasm.length) {
    const id = carrierWasm[offset++];
    const size = decodeUleb(carrierWasm, offset);
    const payloadStart = size.offset;
    const payloadEnd = payloadStart + size.value;
    let payload = carrierWasm.subarray(payloadStart, payloadEnd);
    switch (id) {
      case 0: { // custom: declare the GC feature appended below
        const nameLength = decodeUleb(payload, 0);
        const nameStart = nameLength.offset;
        const name = payload.subarray(nameStart, nameStart + nameLength.value).toString();
        if (name === 'target_features') payload = appendTargetFeature(payload, Buffer.from('gc'));
        if (name === '.debug_info') {
          // LLVM describes the line-aligned carrier as C11. The fixture models
          // the production Waluau policy instead: the CU DIE layout generated
          // above places its DW_AT_language data2 value 16 bytes into the
          // section payload.
          payload = Buffer.from(payload);
          const debugInfoStart = nameStart + nameLength.value;
          const languageOffset = debugInfoStart + 16;
          const carrierLanguage = payload.readUInt16LE(languageOffset);
          if (carrierLanguage !== 0x001d) {
            throw new Error(`expected DW_LANG_C11, found 0x${carrierLanguage.toString(16)}`);
          }
          payload.writeUInt16LE(0x8000, languageOffset);
        }
        break;
      }
      case 1: // type: append (struct (field i32)) as type index 2
        payload = appendVectorEntry(payload, [0x5f, 0x01, 0x7f, 0x00]);
        break;
      case 3: // function: append a function using type index 0
        payload = appendVectorEntry(payload, [0x00]);
        break;
      case 7: // export: expose the synthetic helper to the browser page
        payload = appendVectorEntry(
          payload,
          [...encodeUleb(exportName.length), ...exportName, 0x00, 0x04],
        );
        break;
      case 10: { // code: append a body without rewriting any authored body
        const body = Buffer.from([
          0x00, // no additional locals
          0x20, 0x00, // local.get 0
          0xfb, 0x00, 0x02, // struct.new type 2
          0xfb, 0x02, 0x02, 0x00, // struct.get type 2, field 0
          0x0b, // end
        ]);
        payload = appendVectorEntry(payload, [...encodeUleb(body.length), ...body]);
        break;
      }
    }
    chunks.push(Buffer.from([id]), encodeUleb(payload.length), payload);
    offset = payloadEnd;
  }
  return Buffer.concat(chunks);
}

// This C input is a debug-section carrier, not a Waluau implementation or a
// supported runtime target. Its line numbers deliberately match the authored
// Waluau fixture. The final module is instantiated only in a browser.
const carrier = `/* line 1 */
/* line 2 */
__attribute__((noinline)) int inner(int value) {
  int boxed = value + 1;
  return boxed;
}
/* line 7 */
__attribute__((noinline)) void trap_inner(void) {
  __builtin_trap();
}
/* line 11 */
__attribute__((export_name("run"))) int run(int value) {
  return inner(value);
}
/* line 15 */
__attribute__((export_name("throw_probe"))) void throw_probe(void) {
  trap_inner();
}
`;

try {
  const source = join(scratch, basename('dwarf_chrome_probe.walu'));
  const object = join(scratch, 'carrier.o');
  const carrierWasm = join(scratch, 'carrier.wasm');

  writeFileSync(source, carrier);
  execFileSync(
    clang,
    [
      '--target=wasm32-unknown-unknown',
      '-x',
      'c',
      '-g',
      `-gdwarf-${dwarfVersion}`,
      '-O0',
      '-nostdlib',
      '-fdebug-compilation-dir=.',
      '-c',
      basename(source),
      '-o',
      object,
    ],
    { cwd: scratch, stdio: 'inherit' },
  );
  execFileSync(
    wasmLd,
    ['--no-entry', '--export=run', '--export=throw_probe', object, '-o', carrierWasm],
    { stdio: 'inherit' },
  );
  writeFileSync(output, addGcProbe(readFileSync(carrierWasm)));
  execFileSync(wasmTools, ['validate', '--features', 'all', output], { stdio: 'inherit' });
  process.stdout.write(`${output}\n`);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
