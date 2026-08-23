import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('record (table) function calling', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.status-text')).not.toHaveText(
      'Loading Compiler...',
      { timeout: COMPILER_READY_TIMEOUT },
    );
    // Open file search and load records_sealed_tables conformance file
    await page.getByRole('button', { name: 'Search (Cmd+P)' }).click();
    await expect(page.locator('.search-modal-container')).toBeVisible();
    await page.locator('.search-input-field').fill('records_sealed_tables');
    await page.keyboard.press('Enter');
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );
    await page.getByRole('button', { name: 'Run', exact: true }).click();
  });

  test('renders point_sum and point_mutation signature correctly', async ({ page }) => {
    const sumCard = page.locator('.func-card:has(.func-signature-name:text("point_sum"))');
    await expect(sumCard.locator('.func-signature')).toContainText('point_sum(param0: i32) -> i32');

    const sqlenCard = page.locator('.func-card:has(.func-signature-name:text("sqlen_vec"))');
    await expect(sqlenCard.locator('.func-signature')).toContainText('sqlen_vec(param0: { x: i32, y: i32 }) -> i32');
  });

  test('can invoke function with record parameter and receive result', async ({ page }) => {
    const sqlenCard = page.locator('.func-card:has(.func-signature-name:text("sqlen_vec"))');
    await expect(sqlenCard).toBeVisible();

    // Verify sub-inputs are rendered for the record fields x and y
    const xInput = sqlenCard.locator('.func-input-row:has-text("x (i32)")').locator('.func-input-field');
    const yInput = sqlenCard.locator('.func-input-row:has-text("y (i32)")').locator('.func-input-field');
    await expect(xInput).toBeVisible();
    await expect(yInput).toBeVisible();

    // Enter values: 3 and 4
    await xInput.fill('3');
    await yInput.fill('4');

    // Expected result: 3^2 + 4^2 = 25
    await expect(sqlenCard.locator('.func-result-value.success')).toHaveText('25');
  });

  test('can invoke function returning record and verify formatting', async ({ page }) => {
    const vecCard = page.locator('.func-card:has(.func-signature-name:text-is("vec"))');
    await expect(vecCard).toBeVisible();

    const param0 = vecCard.locator('.func-input-row:has-text("param0 (i32)")').locator('.func-input-field');
    const param1 = vecCard.locator('.func-input-row:has-text("param1 (i32)")').locator('.func-input-field');
    
    await param0.fill('2');
    await param1.fill('5');

    // Expected formatted record output: { x = 2, y = 5 }
    await expect(vecCard.locator('.func-result-value.success')).toHaveText('{ x = 2, y = 5 }');
  });

  test('can invoke incremental record initialization function and get formatted record', async ({ page }) => {
    const incCard = page.locator('.func-card:has(.func-signature-name:text("make_incremental_record"))');
    await expect(incCard).toBeVisible();

    const seedInput = incCard.locator('.func-input-row:has-text("param0 (i32)")').locator('.func-input-field');
    await seedInput.fill('10');

    // Expected formatted record output: { x = 10, y = 11 }
    await expect(incCard.locator('.func-result-value.success')).toHaveText('{ x = 10, y = 11 }');
  });
});
