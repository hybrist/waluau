import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('string function calling', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.status-text')).not.toHaveText(
      'Loading Compiler...',
      { timeout: COMPILER_READY_TIMEOUT },
    );
    // Click on the String Ops preset button.
    await page.getByRole('button', { name: 'String Ops' }).click();
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );
    await page.getByRole('button', { name: 'Run' }).click();
  });

  test('shows greet, concat and equals function cards with string inputs', async ({ page }) => {
    // Assert that the signatures show string type
    const signatures = page.locator('.func-signature');
    await expect(signatures.nth(1)).toContainText('concat');
    await expect(signatures.nth(1)).toContainText('param0: string');
    await expect(signatures.nth(1)).toContainText('param1: string');
    await expect(signatures.nth(1)).toContainText('string');
  });

  test('defaults to empty string literals', async ({ page }) => {
    // Default output should show result for empty string
    // greet("", "") -> "Hello, "
    // Find the greet function card. We can locate by signature name.
    const greetCard = page.locator('.func-card:has(.func-signature-name:text("greet"))');
    await expect(greetCard.locator('.func-result-value.success')).toHaveText('"Hello, "');

    const concatCard = page.locator('.func-card:has(.func-signature-name:text("concat"))');
    await expect(concatCard.locator('.func-result-value.success')).toHaveText('""');

    const equalsCard = page.locator('.func-card:has(.func-signature-name:text("equals"))');
    await expect(equalsCard.locator('.func-result-value.success')).toHaveText('true');
  });

  test('accepts quoted string literals', async ({ page }) => {
    const greetCard = page.locator('.func-card:has(.func-signature-name:text("greet"))');
    await greetCard.locator('.func-input-field').fill('"Alice"');
    await expect(greetCard.locator('.func-result-value.success')).toHaveText('"Hello, Alice"');

    const concatCard = page.locator('.func-card:has(.func-signature-name:text("concat"))');
    const concatFields = concatCard.locator('.func-input-field');
    await concatFields.nth(0).fill('"foo"');
    await concatFields.nth(1).fill('"bar"');
    await expect(concatCard.locator('.func-result-value.success')).toHaveText('"foobar"');
  });

  test('accepts unquoted raw strings', async ({ page }) => {
    const greetCard = page.locator('.func-card:has(.func-signature-name:text("greet"))');
    await greetCard.locator('.func-input-field').fill('Bob');
    await expect(greetCard.locator('.func-result-value.success')).toHaveText('"Hello, Bob"');
  });

  test('decodes escape sequences', async ({ page }) => {
    const concatCard = page.locator('.func-card:has(.func-signature-name:text("concat"))');
    const concatFields = concatCard.locator('.func-input-field');
    await concatFields.nth(0).fill('"a\\nb"');
    await concatFields.nth(1).fill('"c\\td"');
    await expect(concatCard.locator('.func-result-value.success')).toHaveText('"a\\nbc\\td"');
  });
});
