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
    // Switch to Generated IR tab to see the diagnostic.
    await page.getByRole('button', { name: 'Generated IR' }).click();
    await expect(page.locator('.diagnostic-output')).toBeVisible();
  });

  test('line numbers reflect the current line count', async ({ page }) => {
    const threeLineProgram = 'function f(): i32\n    return 1\nend';
    await page.locator('.code-textarea').fill(threeLineProgram);
    await expect(page.locator('.monaco-editor .line-numbers')).toHaveCount(3);
  });

  test('Monaco Editor uses the custom waluau language mode', async ({ page }) => {
    await page.waitForSelector('.monaco-editor');
    const languageId = await page.evaluate(() => {
      return window.monaco?.editor?.getModels()?.[0]?.getLanguageId();
    });
    expect(languageId).toBe('waluau');
  });

  test('entry-point error message appears in the run tab when top-level code traps', async ({ page }) => {
    await page.locator('.code-textarea').fill('assert(false)');
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await expect(page.getByRole('heading', { name: 'Module Load Error' })).toBeVisible();
    await expect(page.locator('.diagnostic-output')).toContainText(
      'Failed to execute the generated WASM module entry point:',
    );
    await expect(page.locator('.diagnostic-output')).not.toContainText('This module requires Wasm GC');
  });

  test('generated Wasm compile failures show the browser diagnostic in the run tab', async ({ page }) => {
    await page.evaluate(() => {
      const originalCompile = WebAssembly.compile;
      WebAssembly.compile = async (...args) => {
        WebAssembly.compile = originalCompile;
        void args;
        throw new WebAssembly.CompileError(
          'simulated mobile Safari rejection at byte 42: ref.cast failed',
        );
      };
    });

    await page.locator('.code-textarea').fill(
      'function identity(values: {i32}): {i32}\n    return values\nend',
    );
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await expect(page.getByRole('heading', { name: 'Module Load Error' })).toBeVisible();
    await expect(page.locator('.diagnostic-output')).toContainText(
      'Failed to compile the generated WASM module: CompileError: simulated mobile Safari rejection at byte 42: ref.cast failed',
    );
    await expect(page.locator('.diagnostic-output')).toContainText(
      'This module uses Wasm GC.',
    );
  });
});
