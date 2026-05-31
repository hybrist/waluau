import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('preset selector', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Wait until the compiler finishes with the default preset before interacting.
    await expect(page.locator('.status-text')).not.toHaveText(
      'Loading Compiler...',
      { timeout: COMPILER_READY_TIMEOUT },
    );
  });

  test('renders a preset button for each fixture', async ({ page }) => {
    // At minimum the fixtures that existed when tests were written should be present.
    const expectedLabels = ['Add', 'Fib', 'Mismatch', 'Control'];
    for (const label of expectedLabels) {
      await expect(page.getByRole('button', { name: label })).toBeVisible();
    }
  });

  test('clicking Fib loads fibonacci source into the editor', async ({ page }) => {
    await page.getByRole('button', { name: 'Fib' }).click();
    await expect(page.locator('.code-textarea')).toContainText('function fib');
  });

  test('Fib preset compiles successfully', async ({ page }) => {
    await page.getByRole('button', { name: 'Fib' }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
  });

  test('Mismatch preset triggers a compilation error', async ({ page }) => {
    await page.getByRole('button', { name: 'Mismatch' }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Failed', {
      timeout: COMPILER_READY_TIMEOUT,
    });
  });

  test('switching back to Add after an error restores success status', async ({ page }) => {
    await page.getByRole('button', { name: 'Mismatch' }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Failed', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    await page.getByRole('button', { name: 'Add' }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
  });

  test('Require Flow Example preset loads and compiles multiple files successfully', async ({ page }) => {
    await page.getByRole('button', { name: 'Require Flow Example' }).click();
    
    await expect(page.locator('.file-item').getByText('main.walu', { exact: true })).toBeVisible();
    await expect(page.locator('.file-item').getByText('double.walu', { exact: true })).toBeVisible();
    await expect(page.locator('.file-item').getByText('add.walu', { exact: true })).toBeVisible();

    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    await page.getByRole('button', { name: 'Generated IR' }).click();
    await expect(page.locator('.ir-output')).toContainText('compute');
  });
});
