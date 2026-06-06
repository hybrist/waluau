import { describe, it, expect } from 'vitest';
import { compileAndInstantiate, compileAndInstantiateWithExports } from '../src/runner.js';

const conformanceModules = import.meta.glob('../../../conformance/**/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default',
});

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
        compileAndInstantiate({ '/main.walu': source }, '/main.walu'),
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
});
