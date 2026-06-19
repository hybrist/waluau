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
      await expect(page.getByRole('button', { name: label, exact: true })).toBeVisible();
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

  test('TFJS Tensor Math preset runs overloaded arithmetic and matmul', async ({ page }) => {
    await page.getByRole('button', { name: 'Tfjs Tensor Math' }).click();

    await expect(page.locator('.code-textarea')).toContainText('local tf = require("tfjs")');
    await expect(page.locator('.code-textarea')).toContainText('local result: Tensor = (a + b) * scale');
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    await page.getByRole('button', { name: 'Run' }).click();

    await expect(
      page.locator('.func-card').filter({ hasText: 'overloaded_vector_result' }).locator('.func-result-value.success'),
    ).toHaveText('66');
    await expect(
      page.locator('.func-card').filter({ hasText: 'matmul_bottom_right' }).locator('.func-result-value.success'),
    ).toHaveText('22');
    await expect(
      page.locator('.func-card').filter({ hasText: 'tidy_memory_delta' }).locator('.func-result-value.success'),
    ).toHaveText('1');
  });

  test('TFJS Model Loading preset loads a local model and writes prediction output', async ({ page }) => {
    await page.getByRole('button', { name: 'Tfjs Model Loading', exact: true }).click();

    await expect(page.locator('.code-textarea')).toContainText('tfjs_load_layers_model');
    await expect(page.locator('.code-textarea')).toContainText('tfjs_dispose_layers_model(model)');
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    await page.getByRole('button', { name: 'Run' }).click();

    await expect(
      page.frameLocator('iframe').locator('#tfjs-model-result').first(),
    ).toHaveText('TFJS model prediction: 13');
  });

  test('global Ctrl+P/Cmd+P opens file search modal and lets user select a file', async ({ page }) => {
    await page.evaluate(() => {
      const event = new KeyboardEvent('keydown', {
        key: 'p',
        code: 'KeyP',
        ctrlKey: true,
        metaKey: true,
        bubbles: true,
        cancelable: true
      });
      document.body.dispatchEvent(event);
    });
    await expect(page.locator('.search-modal-container')).toBeVisible();
    await expect(page.locator('.search-input-field')).toBeFocused();
    
    await page.locator('.search-input-field').fill('arith');
    const results = page.locator('.search-item');
    await expect(results).toHaveCount(1);
    await expect(results.first().locator('.search-item-name')).toHaveText('arithmetic.walu');
    await expect(results.first().locator('.search-category-tag')).toHaveText('Conformance');
    
    await page.keyboard.press('Enter');
    await expect(page.locator('.search-modal-container')).not.toBeVisible();
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
  });

  test('clicking Search button opens file search modal and lets user select a file', async ({ page }) => {
    await page.getByRole('button', { name: 'Search (Cmd+P)' }).click();
    await expect(page.locator('.search-modal-container')).toBeVisible();
    await expect(page.locator('.search-input-field')).toBeFocused();
    
    await page.locator('.search-input-field').fill('arith');
    const results = page.locator('.search-item');
    await expect(results).toHaveCount(1);
    await expect(results.first().locator('.search-item-name')).toHaveText('arithmetic.walu');
    
    await page.keyboard.press('Enter');
    await expect(page.locator('.search-modal-container')).not.toBeVisible();
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
  });
});
