import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const script = path.join(repoRoot, 'tools/dom-idl/generate-dom-externs.mjs');

function readRepoFile(relativePath) {
  return readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

function runGenerator({
  input,
  filter = 'tools/dom-idl/filter.json',
  patches = 'tools/dom-idl/patches.json',
} = {}) {
  const dir = mkdtempSync(path.join(tmpdir(), 'waluau-dom-idl-'));
  const externs = path.join(dir, 'dom.walu');
  const metadata = path.join(dir, 'dom.metadata.json');
  const diagnostics = path.join(dir, 'dom.diagnostics.txt');

  const args = [
    script,
    '--filter',
    filter,
    '--patches',
    patches,
    '--out',
    externs,
    '--metadata-out',
    metadata,
    '--diagnostics-out',
    diagnostics,
  ];
  if (input) {
    args.splice(1, 0, '--input', input);
  }

  execFileSync(process.execPath, args, { cwd: repoRoot });

  return {
    externs: readFileSync(externs, 'utf8'),
    metadata: readFileSync(metadata, 'utf8'),
    diagnostics: readFileSync(diagnostics, 'utf8'),
  };
}

test('DOM extern generation is stable', () => {
  const { externs, metadata, diagnostics } = runGenerator();

  assert.equal(externs, readRepoFile('externs/dom.walu'));
  assert.equal(metadata, readRepoFile('externs/dom.metadata.json'));
  assert.equal(diagnostics, readRepoFile('externs/dom.diagnostics.txt'));
});

test('selected DOM interfaces are emitted in parent-before-child order', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'waluau-dom-idl-order-'));
  const input = path.join(dir, 'custom.webidl');
  const filter = path.join(dir, 'filter.json');
  const patches = path.join(dir, 'patches.json');

  writeFileSync(input, [
    'interface Parent {',
    '};',
    'interface Child : Parent {',
    '  attribute Parent? owner;',
    '};',
    '',
  ].join('\n'));
  writeFileSync(filter, JSON.stringify({
    interfaces: ['Child', 'Parent'],
    typeMap: {},
    disabledMembers: {},
    hostFunctions: [],
    skipIssueRefs: {},
  }));
  writeFileSync(patches, JSON.stringify({}));

  const generated = runGenerator({
    input: path.relative(repoRoot, input),
    filter: path.relative(repoRoot, filter),
    patches: path.relative(repoRoot, patches),
  });
  const inheritance = JSON.parse(generated.metadata).inheritance.map((entry) => entry.interface);

  assert.ok(
    generated.externs.indexOf('type Parent = extern') < generated.externs.indexOf('type Child = extern extends Parent'),
    'parent extern should be emitted before child extern',
  );
  assert.match(generated.externs, /^declare property Child:owner: Parent\?$/m);
  assert.deepEqual(inheritance, ['Parent', 'Child']);
});

test('unsupported Web IDL members are diagnosed deterministically', () => {
  const diagnostics = readRepoFile('externs/dom.diagnostics.txt').trim();
  assert.notEqual(diagnostics, '');
  // The negative filter enables every member of a selected interface by default, so
  // diagnostics now record *why* each unrepresentable or disabled member was skipped --
  // they should be a stable, sorted list of single-line "skip Interface.member: reason" entries.
  for (const line of diagnostics.split('\n')) {
    assert.match(line, /^skip [A-Za-z0-9]+\.[A-Za-z0-9_<>]+: .+$/);
  }
  const sorted = [...diagnostics.split('\n')].sort();
  assert.deepEqual(diagnostics.split('\n'), sorted);
});

test('disabled DOM members document a beads issue reference', () => {
  const metadata = JSON.parse(readRepoFile('externs/dom.metadata.json'));
  const disabled = metadata.skippedMembers.filter((entry) => entry.disabled);
  assert.ok(disabled.length > 0, 'expected the negative filter to disable some members');
  for (const entry of disabled) {
    assert.match(
      entry.issue ?? '',
      /^waluau-[a-z0-9]+$/,
      `${entry.interface}.${entry.member} is explicitly disabled and must reference a beads issue`,
    );
    assert.ok(entry.reason && entry.reason.length > 0, `${entry.interface}.${entry.member} must document a reason`);
  }
});

test('skip diagnostics reference tracked compiler/tooling limitations where applicable', () => {
  const diagnostics = readRepoFile('externs/dom.diagnostics.txt');
  assert.match(diagnostics, /unsupported generic Web IDL type sequence<[^>]*> \(waluau-utyc\)/);
  assert.match(diagnostics, /unsupported generic Web IDL type Promise<[^>]*> \(waluau-2c6n\)/);
  assert.match(diagnostics, /unsupported union Web IDL type \([^)]*\) \(waluau-o4xs\)/);
  assert.match(diagnostics, /nullable callback Web IDL type .+ has no extern syntax representation \(waluau-lqjs\)/);
  assert.match(diagnostics, /nullable modifier rejects primitive Web IDL type .+ \(waluau-pq7p\)/);
  assert.match(diagnostics, /unsupported Web IDL type (any|object\??) \(waluau-lxdd\)/);
  assert.match(diagnostics, /collides with .* \(waluau-1l02\)/);
});

test('generated externs emit DOM inheritance syntax', () => {
  const externs = readRepoFile('externs/dom.walu');
  assert.match(externs, /^type Event = extern$/m);
  assert.match(externs, /^type Node = extern extends EventTarget$/m);
  assert.match(externs, /^type Document = extern extends Node$/m);
  assert.match(externs, /^type Window = extern extends EventTarget$/m);
  assert.match(externs, /^type Element = extern extends Node$/m);
  assert.match(externs, /^type HTMLElement = extern extends Element$/m);
  assert.match(externs, /^type HTMLHeadingElement = extern extends HTMLElement$/m);
});

test('generated DOM externs keep nullable-parent inheritance chains parent-first', () => {
  const externs = readRepoFile('externs/dom.walu');
  const metadata = JSON.parse(readRepoFile('externs/dom.metadata.json'));
  const inheritance = metadata.inheritance.map((entry) => entry.interface);

  assert.ok(
    externs.indexOf('type AbstractRange = extern') < externs.indexOf('type Range = extern extends AbstractRange'),
    'AbstractRange should precede Range in generated externs',
  );
  assert.ok(
    externs.indexOf('type DOMRectReadOnly = extern') < externs.indexOf('type DOMRect = extern extends DOMRectReadOnly'),
    'DOMRectReadOnly should precede DOMRect in generated externs',
  );
  assert.ok(
    externs.indexOf('type StyleSheet = extern') < externs.indexOf('type CSSStyleSheet = extern extends StyleSheet'),
    'StyleSheet should precede CSSStyleSheet in generated externs',
  );
  assert.ok(
    inheritance.indexOf('AbstractRange') < inheritance.indexOf('Range'),
    'metadata should mirror parent-before-child ordering for AbstractRange/Range',
  );
  assert.ok(
    inheritance.indexOf('DOMRectReadOnly') < inheritance.indexOf('DOMRect'),
    'metadata should mirror parent-before-child ordering for DOMRectReadOnly/DOMRect',
  );
  assert.ok(
    inheritance.indexOf('StyleSheet') < inheritance.indexOf('CSSStyleSheet'),
    'metadata should mirror parent-before-child ordering for StyleSheet/CSSStyleSheet',
  );
});

test('generated externs expose the DOM window root', () => {
  const externs = readRepoFile('externs/dom.walu');
  assert.match(externs, /^declare property Window:document: Document$/m);
  assert.match(externs, /^declare function dom_window\(\): Window$/m);
});

test('generated externs expose narrow DOM Promise APIs for fetch and Response.text', () => {
  const externs = readRepoFile('externs/dom.walu');
  const metadata = JSON.parse(readRepoFile('externs/dom.metadata.json'));
  const diagnostics = readRepoFile('externs/dom.diagnostics.txt');

  assert.match(externs, /^type Promise<T> = extern$/m);
  assert.match(externs, /^type Response = extern$/m);
  assert.match(externs, /^declare function Window:fetch\(input: string\): Promise<Response>$/m);
  assert.match(externs, /^declare function Response:text\(\): Promise<string>$/m);
  assert.match(externs, /^declare function fetch\(input: string\): Promise<Response>$/m);
  assert.doesNotMatch(diagnostics, /skip Window\.fetch: unsupported generic Web IDL type Promise<Response>/);
  assert.doesNotMatch(diagnostics, /skip Response\.text: unsupported generic Web IDL type Promise<USVString>/);
  assert.match(diagnostics, /skip Blob\.text: unsupported generic Web IDL type Promise<USVString> \(waluau-2c6n\)/);

  const emittedMembers = metadata.emittedMembers.map((entry) => `${entry.interface}.${entry.idlName}`);
  assert.ok(emittedMembers.includes('Window.fetch'));
  assert.ok(emittedMembers.includes('Response.text'));
  assert.ok(metadata.emittedHostFunctions.some((entry) =>
    entry.name === 'fetch' &&
    entry.returnType === 'Promise<Response>' &&
    entry.params?.join(', ') === 'input: string'
  ));
});

test('DOM Promise generation keeps nested generic Promise returns disabled', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'waluau-dom-idl-promise-nested-'));
  const input = path.join(dir, 'custom.webidl');
  const filter = path.join(dir, 'filter.json');
  const patches = path.join(dir, 'patches.json');

  writeFileSync(input, [
    'partial interface Window {',
    '  Promise<sequence<DOMString>> fetch();',
    '};',
    '',
  ].join('\n'));
  writeFileSync(filter, JSON.stringify({
    interfaces: ['Window'],
    typeMap: {
      DOMString: 'string',
    },
    disabledMembers: {},
    hostFunctions: [],
    skipIssueRefs: {
      'unsupported-generic:Promise': 'waluau-2c6n',
    },
  }));
  writeFileSync(patches, JSON.stringify({}));

  const generated = runGenerator({
    input: path.relative(repoRoot, input),
    filter: path.relative(repoRoot, filter),
    patches: path.relative(repoRoot, patches),
  });

  assert.doesNotMatch(generated.externs, /^declare function Window:fetch/m);
  assert.match(
    generated.diagnostics,
    /skip Window\.fetch: unsupported generic Web IDL type Promise<sequence<DOMString>> \(waluau-2c6n\)/,
  );
});

test('generated externs expose minimal DOM event callbacks', () => {
  const externs = readRepoFile('externs/dom.walu');
  assert.match(externs, /^declare property Event:type: string$/m);
  assert.match(externs, /^declare property Event:target: EventTarget$/m);
  assert.match(externs, /^declare function EventTarget:add_event_listener\(type: string, callback: \(Event\) -> unit\): unit$/m);
});

test('generated externs expose minimal DOM mutation and storage APIs', () => {
  const externs = readRepoFile('externs/dom.walu');
  assert.match(externs, /^type Storage = extern$/m);
  assert.match(externs, /^type HTMLInputElement = extern extends HTMLElement$/m);
  assert.match(externs, /^type HTMLTextAreaElement = extern extends HTMLElement$/m);
  assert.match(externs, /^declare property HTMLElement:value: string$/m);
  assert.match(externs, /^declare property Document:body: HTMLElement$/m);
  assert.match(externs, /^declare property Document:document_element: Element$/m);
  assert.match(externs, /^declare function Node:replace_child\(new_child: Node, old_child: Node\): Node$/m);
  assert.match(externs, /^declare function Node:remove_child\(child: Node\): Node$/m);
  assert.match(externs, /^declare function Element:get_attribute\(name: string\): string\?$/m);
  assert.match(externs, /^declare property Window:local_storage: Storage$/m);
  assert.match(externs, /^declare function Storage:get_item\(key: string\): string\?$/m);
});

test('generated externs expose a focused canvas 2D rendering slice', () => {
  const externs = readRepoFile('externs/dom.walu');
  const metadata = JSON.parse(readRepoFile('externs/dom.metadata.json'));
  const diagnostics = readRepoFile('externs/dom.diagnostics.txt');

  assert.match(externs, /^type HTMLCanvasElement = extern extends HTMLElement$/m);
  assert.match(externs, /^type CanvasRenderingContext2D = extern$/m);
  assert.match(
    externs,
    /^declare function HTMLCanvasElement:get_context\(context_id: string\): CanvasRenderingContext2D\?$/m,
  );
  assert.match(externs, /^declare property CanvasRenderingContext2D:canvas: HTMLCanvasElement$/m);
  assert.match(externs, /^declare function CanvasRenderingContext2D:fillRect\(x: f64, y: f64, w: f64, h: f64\): unit$/m);
  assert.match(externs, /^declare function CanvasRenderingContext2D:clearRect\(x: f64, y: f64, w: f64, h: f64\): unit$/m);
  assert.match(externs, /^declare function CanvasRenderingContext2D:fillText\(text: string, x: f64, y: f64\): unit$/m);
  assert.match(diagnostics, /skip CanvasRenderingContext2D\.drawImage: image: unsupported Web IDL type CanvasImageSource/);

  // waluau-b3hl: the (DOMString or CanvasGradient or CanvasPattern) union is
  // collapsed to `string` through filter.json's unionTypeMap, so the paint
  // style properties are emitted instead of skipped.
  assert.match(externs, /^declare property CanvasRenderingContext2D:fillStyle: string$/m);
  assert.match(externs, /^declare property CanvasRenderingContext2D:strokeStyle: string$/m);
  assert.doesNotMatch(diagnostics, /skip CanvasRenderingContext2D\.fillStyle/);
  assert.doesNotMatch(diagnostics, /skip CanvasRenderingContext2D\.strokeStyle/);
  const emittedCanvasMembers = metadata.emittedMembers.map(
    (entry) => `${entry.interface}.${entry.idlName}`,
  );
  assert.ok(emittedCanvasMembers.includes('CanvasRenderingContext2D.fillStyle'));
  assert.ok(emittedCanvasMembers.includes('CanvasRenderingContext2D.strokeStyle'));

  const getContext = metadata.emittedMembers.find(
    (entry) => entry.interface === 'HTMLCanvasElement' && entry.idlName === 'getContext',
  );
  assert.deepEqual(getContext?.params, ['context_id: string']);
  assert.equal(getContext?.returnType, 'CanvasRenderingContext2D?');
});

test('unionTypeMap collapses exact Web IDL union shapes and skips unmapped unions', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'waluau-dom-idl-union-'));
  const input = path.join(dir, 'custom.webidl');
  const filter = path.join(dir, 'filter.json');
  const patches = path.join(dir, 'patches.json');

  writeFileSync(input, [
    'interface Gradient {',
    '};',
    'interface Pattern {',
    '};',
    'interface Painter {',
    '  attribute (DOMString or Gradient or Pattern) paint;',
    '  attribute (DOMString or Gradient or Pattern)? maybePaint;',
    '  attribute (Gradient or Pattern) unmapped;',
    '  undefined apply((DOMString or Gradient or Pattern) style);',
    '};',
    '',
  ].join('\n'));
  writeFileSync(filter, JSON.stringify({
    interfaces: ['Painter'],
    typeMap: {
      DOMString: 'string',
      undefined: 'unit',
    },
    unionTypeMap: {
      '(DOMString or Gradient or Pattern)': 'string',
    },
    disabledMembers: {},
    hostFunctions: [],
    skipIssueRefs: {
      'unsupported-union': 'waluau-o4xs',
    },
  }));
  writeFileSync(patches, JSON.stringify({}));

  const generated = runGenerator({
    input: path.relative(repoRoot, input),
    filter: path.relative(repoRoot, filter),
    patches: path.relative(repoRoot, patches),
  });

  // A mapped union collapses in attribute, nullable-attribute, and parameter positions.
  assert.match(generated.externs, /^declare property Painter:paint: string$/m);
  assert.match(generated.externs, /^declare property Painter:maybePaint: string\?$/m);
  assert.match(generated.externs, /^declare function Painter:apply\(style: string\): unit$/m);
  // A union without a unionTypeMap entry is still skipped with the tracked issue ref.
  assert.doesNotMatch(generated.externs, /unmapped/);
  assert.match(
    generated.diagnostics,
    /skip Painter\.unmapped: unsupported union Web IDL type \(Gradient or Pattern\) \(waluau-o4xs\)/,
  );
});

test('generated externs sanitize reserved DOM parameter names deterministically', () => {
  const externs = readRepoFile('externs/dom.walu');
  const metadata = JSON.parse(readRepoFile('externs/dom.metadata.json'));
  const diagnostics = readRepoFile('externs/dom.diagnostics.txt');

  assert.match(
    externs,
    /^declare function HTMLInputElement:setRangeText\(replacement: string, start: u32, end_: u32\): unit$/m,
  );
  assert.match(
    externs,
    /^declare function HTMLInputElement:setSelectionRange\(start: u32, end_: u32\): unit$/m,
  );
  assert.match(
    externs,
    /^declare function HTMLTextAreaElement:setRangeText\(replacement: string, start: u32, end_: u32\): unit$/m,
  );
  assert.doesNotMatch(diagnostics, /skip HTMLInputElement\.setRangeText: .*waluau-9szs/);
  assert.doesNotMatch(diagnostics, /skip HTMLTextAreaElement\.setRangeText: .*waluau-9szs/);

  const inputSetRangeText = metadata.emittedMembers.find(
    (entry) => entry.interface === 'HTMLInputElement' && entry.idlName === 'setRangeText',
  );
  assert.deepEqual(inputSetRangeText?.params, ['replacement: string', 'start: u32', 'end_: u32']);
});
