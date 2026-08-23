import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('tagged unions support', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.status-text')).not.toHaveText(
      'Loading Compiler...',
      { timeout: COMPILER_READY_TIMEOUT },
    );
  });

  test('compiles, inputs, and formats tagged unions correctly', async ({ page }) => {
    const code = `type Option = Some(i32) | None(unit)

function make_unit(): unit
end

function process_option(opt: Option): string
    if opt is Some then
        return "Got Some"
    else
        return "Got None"
    end
end

function get_some(): Option
    return Some(42)
end

function get_none(): Option
    return None(make_unit())
end
`;
    await page.locator('.code-textarea').fill(code);
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );

    // Switch to Run tab
    await page.getByRole('button', { name: 'Run', exact: true }).click();

    // Verify Process Option function signature and custom tagged union input layout
    const procCard = page.locator('.func-card:has(.func-signature-name:text-is("process_option"))');
    await expect(procCard).toBeVisible();
    await expect(procCard.locator('.func-signature')).toContainText('process_option(param0: Some(i32) | None(unit)) -> string');

    // Default variant selected should be the first one: Some
    const dropdown = procCard.locator('select');
    await expect(dropdown).toHaveValue('Some');

    // Input for payload is rendered
    const payloadInput = procCard.locator('.func-input-field[placeholder="Enter i32"]');
    await expect(payloadInput).toBeVisible();

    // The output should be "Got Some" because by default it passes Some(0)
    await expect(procCard.locator('.func-result-value.success')).toHaveText('"Got Some"');

    // Change variant to None
    await dropdown.selectOption('None');

    // Input for payload should NOT be visible because None variant has no payload (Unit)
    await expect(payloadInput).not.toBeVisible();

    // Expected result should change to "Got None"
    await expect(procCard.locator('.func-result-value.success')).toHaveText('"Got None"');

    // Now test return value rendering
    const someCard = page.locator('.func-card:has(.func-signature-name:text-is("get_some"))');
    await expect(someCard).toBeVisible();
    await expect(someCard.locator('.func-result-value.success')).toHaveText('Some(42)');

    const noneCard = page.locator('.func-card:has(.func-signature-name:text-is("get_none"))');
    await expect(noneCard).toBeVisible();
    await expect(noneCard.locator('.func-result-value.success')).toHaveText('None()');
  });
});
