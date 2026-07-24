import { readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const vertexShaderName = 'ante.effects.vertex';
export const requiredEffectName = 'ante.effects.defeat-shroud';

// Fragment filenames are the registration interface: adding
// `src/shaders/new-effect.frag` automatically exposes
// `ante.effects.new-effect` to Vite and the behavior tests. There is no
// parallel catalog or total to update.
const shaderDirectory = join(dirname(fileURLToPath(import.meta.url)), 'src', 'shaders');
const fragmentSources = Object.fromEntries(
  readdirSync(shaderDirectory)
    .filter((filename) => filename.endsWith('.frag'))
    .sort()
    .map((filename) => [
      `ante.effects.${filename.slice(0, -'.frag'.length)}`,
      `src/shaders/${filename}`,
    ]),
);

export const shaderSources = Object.freeze({
  [vertexShaderName]: 'src/shaders/effects.vert',
  ...fragmentSources,
});
