const fixtureModules = import.meta.glob('../../../../fixtures/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const moduleFixtures = import.meta.glob('../../../../fixtures/modules/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const conformanceModules = import.meta.glob('../../../../conformance/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

export { fixtureModules, moduleFixtures, conformanceModules };

const SINGLE_PRESETS = Object.entries(fixtureModules)
  .map(([path, source]) => {
    const filename = path.split('/').pop();
    const key = filename.replace(/\.walu$/, '');
    const label = key
      .split('-')
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(' ');
    
    return {
      key,
      label,
      files: {
        [`/${filename}`]: source
      },
      entryFile: `/${filename}`
    };
  });

const CONFORMANCE_PRESETS = Object.entries(conformanceModules)
  .map(([path, source]) => {
    const filename = path.split('/').pop();
    const key = filename.replace(/\.walu$/, '');
    const label = key
      .split('_')
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(' ');
    
    return {
      key: `conformance-${key}`,
      label: `${label} (Test)`,
      files: {
        [`/${filename}`]: source
      },
      entryFile: `/${filename}`
    };
  });

export const MULTI_PRESET = {
  key: 'require-flow',
  label: 'Require Flow Example',
  files: Object.entries(moduleFixtures).reduce((acc, [path, source]) => {
    const filename = path.split('/').pop();
    acc[`/${filename}`] = source;
    return acc;
  }, {}),
  entryFile: '/main.walu'
};

export const PRESETS = [...SINGLE_PRESETS, MULTI_PRESET, ...CONFORMANCE_PRESETS].sort((left, right) =>
  left.label.localeCompare(right.label)
);

export const DEFAULT_PRESET = PRESETS[0] || {
  key: 'default',
  label: 'Default',
  files: { '/main.walu': '' },
  entryFile: '/main.walu'
};
