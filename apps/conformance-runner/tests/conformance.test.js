import { describe, it, expect } from 'vitest';
import {
  compileAndInstantiate,
  compileAndInstantiateWithDom,
  compileAndInstantiateWithExports,
} from '../src/runner.js';
import { conformanceIncludePaths } from '../../../tools/conformance/includes.js';

const conformanceModules = import.meta.glob('../../../conformance/**/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default',
});

const includeModules = import.meta.glob('../../../{builtins,externs}/**/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default',
});

function sourceForCase(testCase) {
  const includes = [];
  for (const resolved of conformanceIncludePaths(testCase.name, testCase.source)) {
    const globKey = `../../..${resolved}`;
    const source = includeModules[globKey];
    if (source === undefined) {
      throw new Error(`Unknown conformance include resolved to ${resolved}`);
    }
    includes.push(source);
  }
  if (includes.length === 0) {
    return testCase.source;
  }
  return `${includes.join('\n')}\n${testCase.source}`;
}

const cases = Object.entries(conformanceModules)
  .map(([path, source]) => {
    const normalized = path.replace(/^.*\/conformance\//, '');
    return { name: normalized, source };
  })
  .sort((a, b) => a.name.localeCompare(b.name));

describe('browser conformance', () => {
  for (const { name, source } of cases) {
    it(`passes ${name}`, async () => {
      await expect(
        compileAndInstantiate({ '/main.walu': sourceForCase({ name, source }) }, '/main.walu'),
      ).resolves.toBeUndefined();
    });
  }

  it('passes extern_host_object.walu round-trip identity checks', async () => {
    const source = cases.find(({ name }) => name === 'extern_host_object.walu').source;
    const exports = await compileAndInstantiateWithExports({ '/main.walu': source }, '/main.walu');
    const element = { id: 'root' };

    expect(exports.identity(element)).toBe(element);
    expect(exports.pass_back(exports.identity(element))).toBe(element);
  });

  it('passes nullable_extern_host_object.walu null and non-null paths', async () => {
    const source = cases.find(({ name }) => name === 'nullable_extern_host_object.walu').source;
    const exports = await compileAndInstantiateWithExports({ '/main.walu': source }, '/main.walu');
    const element = { id: 'root' };

    expect(exports.nullable_score(null)).toBe(10);
    expect(exports.nullable_score(element)).toBe(20);
    expect(exports.nullable_eq_score(null)).toBe(30);
    expect(exports.nullable_eq_score(element)).toBe(20);
  });

  it('renders DOM extern handles into the conformance DOM root', async () => {
    const testCase = cases.find(({ name }) => name === 'dom_extern_rendering.walu');
    const source = sourceForCase(testCase);
    const { root, cleanup } = await compileAndInstantiateWithDom({ '/main.walu': source }, '/main.walu');

    try {
      expect(root.children).toHaveLength(2);
      expect(root.children[0].tagName).toBe('H1');
      expect(root.children[0].id).toBe('generated-heading');
      expect(root.children[0].className).toBe('title');
      expect(root.children[0].textContent).toBe('Hello from generated DOM externsleaf');
      expect(root.children[0].children).toHaveLength(1);
      expect(root.children[0].children[0].tagName).toBe('SPAN');
      expect(root.children[0].children[0].className).toBe('leaf');
      expect(root.children[0].children[0].textContent).toBe('leaf');
      expect(root.children[1].tagName).toBe('P');
      expect(root.children[1].id).toBe('generated-paragraph');
      expect(root.children[1].className).toBe('body');
      expect(root.children[1].textContent).toBe('Rendered through generated extern DOM handles');
    } finally {
      cleanup();
    }
  });
});
