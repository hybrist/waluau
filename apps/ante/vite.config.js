import { defineConfig } from 'vite';
import { waluau } from '@waluau/vite-plugin';

export default defineConfig({
  plugins: [waluau()],
});
