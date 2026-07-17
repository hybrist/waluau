// The Arcane Heist example, defined independently of the playground UI so the
// standalone /output/poker-tricks page can render it without pulling in the
// editor, the preset list, or the rest of the fixtures.

import cardBackUrl from '../../../../fixtures/poker-tricks/assets/card-back.svg?url';
import vaultFontUrl from '../../../../fixtures/poker-tricks/assets/Cinzel-Bold.ttf?url';

const engineModules = import.meta.glob('../../../../engine/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const pokerTricksModules = import.meta.glob('../../../../fixtures/poker-tricks/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

function filesUnder(modules, directory) {
  return Object.entries(modules).reduce((acc, [path, source]) => {
    acc[`${directory}/${path.split('/').pop()}`] = source;
    return acc;
  }, {});
}

export const POKER_TRICKS_EXAMPLE = {
  key: 'poker-tricks',
  label: 'Arcane Heist',
  files: {
    ...filesUnder(engineModules, '/engine'),
    ...filesUnder(pokerTricksModules, '/fixtures/poker-tricks')
  },
  entryFile: '/fixtures/poker-tricks/main.walu',
  assetManifest: {
    'assets/card-back.svg': { url: cardBackUrl, type: 'image' },
    'assets/Cinzel-Bold.ttf': { url: vaultFontUrl, type: 'font' },
  },
};
