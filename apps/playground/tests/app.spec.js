import { test, expect } from '@playwright/test';

// Time to wait for the WASM compiler to initialize and produce output.
const COMPILER_READY_TIMEOUT = 20_000;

test.describe('page load', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
  });

  test('has the correct document title', async ({ page }) => {
    await expect(page).toHaveTitle('Waluau Compiler Playground');
  });

  test('shows the brand heading and subtitle', async ({ page }) => {
    await expect(page.getByRole('heading', { name: 'Waluau', level: 1 })).toBeVisible();
    await expect(page.getByText('Compiler Playground & Wasm Codegen')).toBeVisible();
  });

  test('status badge transitions to success once the default preset compiles', async ({ page }) => {
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );
  });

  test('editor is pre-filled with the default preset source', async ({ page }) => {
    // "Add" is first alphabetically and therefore the default preset.
    await expect(page.locator('.code-textarea')).toContainText('function add');
  });

  test('four output tabs are visible', async ({ page }) => {
    for (const label of ['Generated IR', 'Wasm Text (WAT)', 'Compiler Diagnostics', 'Function Calling']) {
      await expect(page.getByRole('button', { name: label })).toBeVisible();
    }
  });
});
