import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('nullable function calling', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.status-text')).not.toHaveText(
      'Loading Compiler...',
      { timeout: COMPILER_READY_TIMEOUT },
    );
    await page.getByRole('button', { name: 'Search (Cmd+P)' }).click();
    await page.locator('.search-input-field').fill('optional_trailing_nullable_args');
    await page.keyboard.press('Enter');
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );
    await page.getByRole('button', { name: 'Run', exact: true }).click();
  });

  test('renders nullable metadata and defaults optional values to nil', async ({ page }) => {
    const chooseCard = page.locator('.func-card:has(.func-signature-name:text-is("choose"))');
    await expect(chooseCard.locator('.func-signature')).toContainText('param1: string?');
    await expect(chooseCard.locator('.func-signature')).not.toContainText('unknown');
    await expect(chooseCard.locator('.func-result-value.success')).toHaveText('""');

    const nullableToggle = chooseCard.locator('label:has-text("Provide value") input[type="checkbox"]');
    await expect(nullableToggle).not.toBeChecked();
  });

  test('can switch a nullable string from nil to a concrete value', async ({ page }) => {
    const chooseCard = page.locator('.func-card:has(.func-signature-name:text-is("choose"))');
    const nullableToggle = chooseCard.locator('label:has-text("Provide value") input[type="checkbox"]');
    await nullableToggle.check();
    await chooseCard.locator('.func-input-field').last().fill('chosen');
    await expect(chooseCard.locator('.func-result-value.success')).toHaveText('"chosen"');
  });

  test('renders a nullable options record instead of unknown', async ({ page }) => {
    const renderCard = page.locator('.func-card:has(.func-signature-name:text-is("render"))');
    await expect(renderCard.locator('.func-signature')).toContainText('param0: {');
    await expect(renderCard.locator('.func-signature')).toContainText('id: string?');
    await expect(renderCard.locator('.func-signature')).not.toContainText('unknown');
    await expect(renderCard.locator('.func-result-value.success')).toHaveText('"default"');
  });
});
