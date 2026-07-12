import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('function calling tab', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Use the default "add" preset (first alphabetically).
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );
    await page.getByRole('button', { name: 'Run' }).click();
  });

  test('shows the add function card with two parameter inputs', async ({ page }) => {
    await expect(page.locator('.func-card')).toBeVisible();
    await expect(page.locator('.func-signature-name').first()).toHaveText('add');
    await expect(page.locator('.func-input-field')).toHaveCount(2);
  });

  test('auto-run computes result for default zero inputs', async ({ page }) => {
    // Both parameters default to 0, so add(0, 0) = 0.
    await expect(page.locator('.func-result-value.success')).toHaveText('0');
  });

  test('auto-run updates result when inputs change', async ({ page }) => {
    const inputs = page.locator('.func-input-field');
    await inputs.nth(0).fill('5');
    await inputs.nth(1).fill('3');
    await expect(page.locator('.func-result-value.success')).toHaveText('8');
  });

  test('disabling auto-run shows idle state until Run Function is clicked', async ({ page }) => {
    // Uncheck the auto-run toggle.
    await page.locator('label:has-text("Auto-run on input change") input[type="checkbox"]').uncheck();
    await expect(page.getByRole('button', { name: 'Run Function' })).toBeVisible();
    await expect(page.locator('.func-result-value.idle')).toBeVisible();

    // Clicking the button should execute and show a result.
    await page.getByRole('button', { name: 'Run Function' }).click();
    await expect(page.locator('.func-result-value.success')).toBeVisible();
  });

  test('invalid input type shows an execution error', async ({ page }) => {
    const inputs = page.locator('.func-input-field');
    await inputs.nth(0).fill('not-a-number');
    // The result box should switch to the error state.
    await expect(page.locator('.func-result-value.error')).toBeVisible();
  });

  test('shows print output during top-level and function execution', async ({ page }) => {
    await page.getByRole('button', { name: 'Top_level_statements' }).click();
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );
    await page.getByRole('button', { name: 'Run' }).click();

    // Verify top-level print output is outside of individual function cards
    await expect(page.locator('.init-logs-box')).toBeVisible();
    await expect(page.locator('.init-logs-value')).toHaveText('Init statement run\nFunction called');

    // Verify the answer function card shows print output from its own execution
    const funcCard = page.locator('.func-card').filter({ hasText: 'answer' });
    await expect(funcCard).toBeVisible();
    await expect(funcCard.locator('.func-logs-box')).toBeVisible();
    await expect(funcCard.locator('.func-logs-value')).toHaveText('Function called');
  });
});
