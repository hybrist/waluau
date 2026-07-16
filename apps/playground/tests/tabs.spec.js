import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('output tabs', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Default preset (Add) must compile before tab content is meaningful.
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );
  });

  test('Run tab is active by default and lists exported functions', async ({ page }) => {
    await expect(page.getByRole('button', { name: 'Run' })).toHaveClass(/active/);
    await expect(page.locator('.func-card')).toBeVisible();
    await expect(page.locator('.func-signature-name').first()).toHaveText('add');
  });

  test('IR tab shows compiler output when clicked', async ({ page }) => {
    await page.getByRole('button', { name: 'Generated IR' }).click();
    await expect(page.getByRole('button', { name: 'Generated IR' })).toHaveClass(/active/);
    await expect(page.locator('.ir-output')).toBeVisible();
    // IR output should be non-empty after a successful compile.
    await expect(page.locator('.ir-output')).not.toHaveText(
      '-- No IR output generated. Write some valid code first.',
    );
  });

  test('WAT tab displays WebAssembly text module', async ({ page }) => {
    await page.getByRole('button', { name: 'Wasm Text (WAT)' }).click();
    await expect(page.locator('.wat-output')).toBeVisible();
    await expect(page.locator('.wat-output')).toContainText('(module');
  });

  test('Compiler Diagnostics tab reports build success', async ({ page }) => {
    await page.getByRole('button', { name: 'Compiler Diagnostics' }).click();
    await expect(page.getByText('Build Succeeded')).toBeVisible();
  });

  test('Compiler Diagnostics tab shows error details after a failed compile', async ({ page }) => {
    // Switch to a preset that intentionally fails.
    await page.getByRole('button', { name: 'Mismatch', exact: true }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Failed', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    await page.getByRole('button', { name: 'Compiler Diagnostics' }).click();
    await expect(page.getByText('Build Failed')).toBeVisible();
    await expect(page.locator('.error-block')).toBeVisible();
  });

  test('Run tab lists exported functions', async ({ page }) => {
    await page.getByRole('button', { name: 'Run' }).click();
    await expect(page.locator('.func-card')).toBeVisible();
    await expect(page.locator('.func-signature-name').first()).toHaveText('add');
  });
});
