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
  const diagnostics = readRepoFile('externs/dom.diagnostics.txt').trim().split('\n');
  assert.deepEqual(diagnostics, [...diagnostics].sort());
  assert.match(diagnostics.join('\n'), /skip EventTarget\.addEventListener: callback: unsupported Web IDL type EventListener/);
});

test('generated externs emit DOM inheritance syntax', () => {
  const externs = readRepoFile('externs/dom.walu');
  assert.match(externs, /^type Node = extern extends EventTarget$/m);
  assert.match(externs, /^type Document = extern extends Node$/m);
  assert.match(externs, /^type Window = extern$/m);
  assert.match(externs, /^type Element = extern extends Node$/m);
  assert.match(externs, /^type HTMLElement = extern extends Element$/m);
  assert.match(externs, /^type HTMLHeadingElement = extern extends HTMLElement$/m);
});

test('generated externs expose the DOM window root', () => {
  const externs = readRepoFile('externs/dom.walu');
  assert.match(externs, /^declare property Window:document: Document$/m);
  assert.match(externs, /^declare function dom_window\(\): Window$/m);
});
