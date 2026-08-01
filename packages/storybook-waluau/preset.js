// Storybook framework preset. Storybook owns the manager, the index, the URL
// and the addons; this preset only teaches it three things: build with Vite
// through the Waluau plugin, index *.stories.walu files, and render them with
// the Waluau renderer.
import { readFile } from 'node:fs/promises';

import { waluau } from '@waluau/vite-plugin';
import { parseStories } from '@waluau/vite-plugin/stories';

export const core = {
  builder: import.meta.resolve('@storybook/builder-vite'),
  renderer: import.meta.resolve('@waluau/storybook/renderer-preset'),
};

/** `framework: { name: '@waluau/storybook', options: { waluau: {...} } }` in main.js. */
async function frameworkOptions(presets) {
  const framework = await presets.apply('framework', {});
  if (typeof framework === 'string' || framework == null) return {};
  return framework.options ?? {};
}

/**
 * Story names come out of the source, not out of a running module: Storybook
 * builds its index in Node, and the CSF module the Vite plugin generates for
 * the same file exports exactly these stories under exactly these names.
 */
export const experimental_indexers = async (existingIndexers) => [
  {
    test: /\.stories\.walu$/,
    createIndex: async (fileName, { makeTitle }) => {
      const source = await readFile(fileName, 'utf8');
      return parseStories(source).map(({ name, exportName }) => ({
        type: 'story',
        importPath: fileName,
        exportName,
        name,
        title: makeTitle(),
      }));
    },
  },
  ...(existingIndexers ?? []),
];

export const viteFinal = async (config, { presets }) => {
  const { waluau: waluauOptions } = await frameworkOptions(presets);
  // Storybook owns this Vite config, including how Waluau is compiled for it.
  // An app's own vite.config.js is loaded by the builder and would otherwise
  // bring its game configuration along — including the full-screen viewport
  // styles, which in a preview iframe would stretch every story's canvas over
  // Storybook's own layout. Compiler options belong in `framework.options`.
  const plugins = (config.plugins ?? [])
    .flat(Infinity)
    .filter((plugin) => plugin?.name !== 'waluau-game');
  return {
    ...config,
    plugins: [...plugins, waluau({ ...(waluauOptions ?? {}), fullScreen: false })],
  };
};
