import { shaderSources } from '../shader-sources.js';

/** @type {import('@waluau/storybook').StorybookConfig} */
export default {
  stories: ['../src/**/*.stories.walu'],
  framework: {
    name: '@waluau/storybook',
    // The same compiler inputs the game is built with: stories draw with the
    // real shaders and the real packaged assets, or they are not showing the
    // game's presentation.
    options: {
      waluau: {
        manifest: 'waluau.assets.json',
        shaderSources,
      },
    },
  },
};
