const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');

const extensionRoot = path.join(__dirname, '..');
const readJson = (relativePath) =>
  JSON.parse(fs.readFileSync(path.join(extensionRoot, relativePath), 'utf8'));

const manifest = readJson('package.json');
const grammar = readJson('syntaxes/waluau.tmLanguage.json');

test('registers .walu files as the Waluau language with its grammar', () => {
  const language = manifest.contributes.languages.find(({ id }) => id === 'waluau');
  assert.ok(language);
  assert.ok(language.extensions.includes('.walu'));
  assert.equal(language.configuration, './language-configuration.json');

  const contribution = manifest.contributes.grammars.find(
    ({ language: id }) => id === 'waluau',
  );
  assert.deepEqual(contribution, {
    language: 'waluau',
    scopeName: 'source.waluau',
    path: './syntaxes/waluau.tmLanguage.json',
  });
  assert.ok(manifest.activationEvents.includes('onLanguage:waluau'));
  assert.ok(!manifest.activationEvents.includes('onLanguage:lua'));

  const workspaceSettings = fs.readFileSync(
    path.join(extensionRoot, '..', '..', 'settings.json'),
    'utf8',
  );
  assert.match(workspaceSettings, /"\*\.walu"\s*:\s*"waluau"/);
  assert.match(
    workspaceSettings,
    /"\[waluau\]"\s*:\s*\{[^}]*"editor\.defaultFormatter"\s*:\s*"waluau-dev\.waluau-vscode"[^}]*\}/,
  );
});

test('layers Waluau syntax rules over the Lua grammar', () => {
  assert.equal(grammar.scopeName, 'source.waluau');
  assert.ok(grammar.patterns.some(({ include }) => include === 'source.lua'));

  const declarationPatterns = grammar.repository['type-declarations'].patterns;
  const typeDeclaration = new RegExp(declarationPatterns[0].match);
  const enumDeclaration = new RegExp(declarationPatterns[1].match);
  assert.ok(typeDeclaration.test('export type State'));
  assert.ok(typeDeclaration.test('export opaque type Handle'));
  assert.ok(enumDeclaration.test('enum Direction'));
  assert.ok(enumDeclaration.test('export enum Direction'));

  const keyword = new RegExp(grammar.repository.keywords.patterns[0].match);
  for (const value of ['case', 'const', 'continue', 'declare', 'export', 'match', 'property']) {
    assert.ok(keyword.test(value), `expected ${value} to be a Waluau keyword`);
  }

  const primitive = new RegExp(grammar.repository['primitive-types'].patterns[0].match);
  for (const value of [
    'number',
    'u32',
    'u64',
    'i32',
    'i64',
    'f32',
    'f64',
    'unit',
    'void',
    'bool',
    'unknown',
    'string',
    'bytes',
    'extern',
    'thread',
    'enum',
  ]) {
    assert.ok(primitive.test(value), `expected ${value} to be a Waluau type`);
  }

  const operator = new RegExp(grammar.repository.operators.patterns[0].match);
  for (const value of ['+=', '//', '//=', '..=', '->', '::', '?', '&', '|', 'is']) {
    assert.ok(operator.test(value), `expected ${value} to be a Waluau operator`);
  }
});
