// The Arcane Heist example, defined independently of the playground UI so the
// standalone /output/poker-tricks page can render it without pulling in the
// editor, the preset list, or the rest of the fixtures.

import cardBackUrl from '../../../ante/assets/card-back.svg?url';
import vaultFontUrl from '../../../ante/assets/Cinzel-Bold.ttf?url';
import cardFlipUrl from '../../../ante/assets/card-flip.wav?url';
import { createAnteProject } from '../../../ante/project.js';

const engineModules = import.meta.glob('../../../../engine/*.walu', {
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

const anteProject = createAnteProject('/fixtures/poker-tricks');

export const POKER_TRICKS_EXAMPLE = {
  key: 'poker-tricks',
  label: 'Arcane Heist',
  files: {
    ...filesUnder(engineModules, '/engine'),
    ...anteProject.files,
  },
  entryFile: anteProject.entryFile,
  assetManifest: {
    'assets/card-back.svg': { url: cardBackUrl, type: 'image' },
    'assets/Cinzel-Bold.ttf': { url: vaultFontUrl, type: 'font' },
    'assets/card-flip.wav': { url: cardFlipUrl, type: 'audio' },
  },
};
