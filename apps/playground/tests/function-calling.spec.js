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
    await page.getByRole('button', { name: 'Run', exact: true }).click();
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

  test('does not show a stale auto-run result when auto-run is re-enabled', async ({ page }) => {
    const inputs = page.locator('.func-input-field');
    await inputs.nth(0).fill('5');
    await inputs.nth(1).fill('3');
    await expect(page.locator('.func-result-value.success')).toHaveText('8');

    const autoRunToggle = page.locator(
      'label:has-text("Auto-run on input change") input[type="checkbox"]'
    );
    await autoRunToggle.uncheck();
    await inputs.nth(0).fill('9');
    await inputs.nth(1).fill('1');
    await expect(page.locator('.func-result-value.idle')).toBeVisible();

    await page.evaluate(() => {
      const resultBox = document.querySelector('.func-result-box');
      window.__autoRunResultTexts = [];
      window.__autoRunResultObserver = new MutationObserver(() => {
        window.__autoRunResultTexts.push(resultBox.textContent);
      });
      window.__autoRunResultObserver.observe(resultBox, {
        childList: true,
        subtree: true,
        characterData: true,
      });
    });

    await autoRunToggle.check();
    await expect(page.locator('.func-result-value.success')).toHaveText('10');
    const observedTexts = await page.evaluate(() => {
      window.__autoRunResultObserver.disconnect();
      return window.__autoRunResultTexts;
    });
    expect(observedTexts.some((text) => text.endsWith('8'))).toBe(false);
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
    await page.getByRole('button', { name: 'Run', exact: true }).click();

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
