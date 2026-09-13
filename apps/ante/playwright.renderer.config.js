import { defineConfig, devices } from '@playwright/test';
import gameplay from './playwright.config.js';

// The production preview setup is shared; renderer contracts own a separate
// test directory and never run in the semantic gameplay command.
export default defineConfig({
  ...gameplay,
  testDir: './renderer',
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
