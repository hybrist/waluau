// Stable project interface for hosts that embed Arcane Heist from source.
// Callers choose a logical root; this adapter owns production-module discovery
// so adding, removing, or reorganizing internal Waluau modules does not require
// updates across conformance and playground consumers.

const productionModules = import.meta.glob([
  './src/**/*.walu',
  '!./src/**/*.test.walu',
], {
  eager: true,
  query: '?raw',
  import: 'default',
});

export function createAnteProject(directory) {
  const root = directory.endsWith('/') ? directory.slice(0, -1) : directory;
  const files = Object.fromEntries(
    Object.entries(productionModules).map(([path, source]) => [
      `${root}/${path.slice('./src/'.length)}`,
      source,
    ]),
  );

  return {
    files,
    entryFile: `${root}/main.walu`,
  };
}
