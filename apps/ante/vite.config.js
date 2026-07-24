import { defineConfig } from 'vite';
import { playwright } from '@vitest/browser-playwright';
import { waluau } from '@waluau/vite-plugin';

export default defineConfig({
  plugins: [waluau({
    manifest: 'waluau.assets.json',
    shaderSources: {
      'ante.effects.vertex': 'src/shaders/effects.vert',
      'ante.effects.gold-shimmer': 'src/shaders/gold-shimmer.frag',
      'ante.effects.defeat-shroud': 'src/shaders/defeat-shroud.frag',
      'ante.effects.red-fire': 'src/shaders/red-fire.frag',
      'ante.effects.blue-caustics': 'src/shaders/blue-caustics.frag',
      'ante.effects.green-growth': 'src/shaders/green-growth.frag',
      'ante.effects.black-hole': 'src/shaders/black-hole.frag',
      'ante.effects.ice-freeze': 'src/shaders/ice-freeze.frag',
    },
  })],
  test: {
    include: ['src/**/*.test.walu'],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: [{ browser: 'chromium' }],
    },
  },
});
