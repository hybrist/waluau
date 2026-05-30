import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('editor', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.status-text')).not.toHaveText(
      'Loading Compiler...',
      { timeout: COMPILER_READY_TIMEOUT },
    );
  });

  test('replacing editor content with valid code compiles successfully', async ({ page }) => {
    await page.locator('.code-textarea').fill(
      'function answer(): i32\n    return 42\nend',
    );
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
  });

  test('invalid code triggers a compilation failure', async ({ page }) => {
    await page.locator('.code-textarea').fill('this is not valid waluau code!!!');
    await expect(page.locator('.status-text')).toHaveText('Compilation Failed', {
      timeout: COMPILER_READY_TIMEOUT,
    });
  });

  test('compilation error message appears in the IR tab error state', async ({ page }) => {
    await page.locator('.code-textarea').fill('???');
    await expect(page.locator('.status-text')).toHaveText('Compilation Failed', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    // The IR tab (active by default) should show the diagnostic.
    await expect(page.locator('.diagnostic-output')).toBeVisible();
  });

  test('line numbers reflect the current line count', async ({ page }) => {
    const threeLineProgram = 'function f(): i32\n    return 1\nend';
    await page.locator('.code-textarea').fill(threeLineProgram);
    await expect(page.locator('.monaco-editor .line-numbers')).toHaveCount(3);
  });
});
