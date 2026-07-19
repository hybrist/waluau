import { fileURLToPath } from 'node:url';

import { waluau } from '../../packages/vite-plugin-waluau/index.js';

export default {
  resolve: {
    alias: {
      '@waluau/vite-plugin/hot': fileURLToPath(
        new URL('../../packages/vite-plugin-waluau/hot.js', import.meta.url),
      ),
      '@waluau/vite-plugin/runtime': fileURLToPath(
        new URL('../../packages/vite-plugin-waluau/runtime.js', import.meta.url),
      ),
    },
  },
  plugins: [
    waluau({
      manifest: 'waluau.assets.json',
      workspaceRoot: new URL('../..', import.meta.url).pathname,
    }),
  ],
};
