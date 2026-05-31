import { describe, it, expect } from 'vitest';
import { compileAndInstantiate } from '../src/runner.js';

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
});
