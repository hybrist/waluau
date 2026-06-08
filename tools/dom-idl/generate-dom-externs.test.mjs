import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const script = path.join(repoRoot, 'tools/dom-idl/generate-dom-externs.mjs');

function readRepoFile(relativePath) {
  return readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

test('DOM extern generation is stable', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'waluau-dom-idl-'));
  const externs = path.join(dir, 'dom.walu');
  const metadata = path.join(dir, 'dom.metadata.json');
  const diagnostics = path.join(dir, 'dom.diagnostics.txt');

  execFileSync(process.execPath, [
    script,
    '--out',
    externs,
    '--metadata-out',
    metadata,
    '--diagnostics-out',
    diagnostics,
  ], { cwd: repoRoot });

  assert.equal(readFileSync(externs, 'utf8'), readRepoFile('externs/dom.walu'));
  assert.equal(readFileSync(metadata, 'utf8'), readRepoFile('externs/dom.metadata.json'));
  assert.equal(readFileSync(diagnostics, 'utf8'), readRepoFile('externs/dom.diagnostics.txt'));
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
  assert.match(diagnostics, /unsupported union Web IDL type \([^)]*\) \(waluau-b3hl\)/);
  assert.match(diagnostics, /nullable callback Web IDL type .+ has no extern syntax representation \(waluau-lqjs\)/);
  assert.match(diagnostics, /nullable modifier rejects primitive Web IDL type .+ \(waluau-pq7p\)/);
  assert.match(diagnostics, /becomes reserved word '[a-z_]+' after snake_case conversion \(waluau-9szs\)/);
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

test('generated externs expose the DOM window root', () => {
  const externs = readRepoFile('externs/dom.walu');
  assert.match(externs, /^declare property Window:document: Document$/m);
  assert.match(externs, /^declare function dom_window\(\): Window$/m);
});

test('generated externs expose minimal DOM event callbacks', () => {
  const externs = readRepoFile('externs/dom.walu');
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
