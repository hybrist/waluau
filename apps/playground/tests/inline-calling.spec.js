import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;
const UNRELATED_EXPORT_SAMPLE = `math.randomseed(1)

function unrelated(): f64
    return math.random()
end

function inspect(): i32
    if math.random() > 0.5 then
        return 0::i32
    end
    return 1::i32
end
`;

test.describe('inline function calling', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Use the default "add" preset (first alphabetically).
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );
  });

  test('clicking CodeLens opens inline runner and executes function', async ({ page }) => {
    // Wait for the Monaco CodeLens to be rendered and visible
    const codeLens = page.locator('.codelens-decoration:has-text("▶ Run add")');
    await expect(codeLens).toBeVisible({ timeout: 10_000 });

    // Click on the CodeLens to open the inline runner
    await codeLens.click();

    // Verify the inline runner widget is displayed
    const widget = page.locator('.inline-runner-widget');
    await expect(widget).toBeVisible();
    await expect(widget.locator('.inline-runner-name')).toHaveText('add');

    // The inline runner should have 2 input fields
    const inputs = widget.locator('.inline-runner-field');
    await expect(inputs).toHaveCount(2);

    // By default, inputs are 0, so result should be 0
    await expect(widget.locator('.result-value.success')).toHaveText('0');

    // Fill inputs and check if result updates
    await inputs.nth(0).fill('12');
    await inputs.nth(1).fill('23');
    await expect(widget.locator('.result-value.success')).toHaveText('35');

    // Click the CodeLens again to close the runner
    const closeLens = page.locator('.codelens-decoration:has-text("▶ Run add")');
    await expect(closeLens).toBeVisible();
    await closeLens.click();

    // Verify the widget is removed
    await expect(widget).not.toBeVisible();
  });

  test('closing inline runner via the close button works', async ({ page }) => {
    const codeLens = page.locator('.codelens-decoration:has-text("▶ Run add")');
    await expect(codeLens).toBeVisible({ timeout: 10_000 });
    await codeLens.click();

    const widget = page.locator('.inline-runner-widget');
    await expect(widget).toBeVisible();

    // Monaco overlays can interfere with pointer-based clicks in CI.
    // Trigger a direct DOM click to verify close-button behavior deterministically.
    const closeButton = widget.locator('.inline-runner-close');
    await expect(closeButton).toBeVisible();
    await closeButton.evaluate((button) => button.click());

    // Verify the widget is removed
    await expect(widget).toBeHidden({ timeout: 10_000 });
  });

  test('executes function inside inline runner for top level statements preset', async ({ page }) => {
    await page.getByRole('button', { name: 'Top_level_statements' }).click();
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );

    // Wait for the Monaco CodeLens to be rendered and visible
    const codeLens = page.locator('.codelens-decoration:has-text("▶ Run answer")');
    await expect(codeLens).toBeVisible({ timeout: 10_000 });
    await codeLens.click();

    // Verify the inline runner widget is displayed
    const widget = page.locator('.inline-runner-widget');
    await expect(widget).toBeVisible();
    await expect(widget.locator('.inline-runner-name')).toHaveText('answer');

    // The inline runner should auto-run by default, displaying 42
    await expect(widget.locator('.result-value.success')).toHaveText('42');
    // Verify there are no logs printed in the inline widget (as requested by user)
    await expect(widget.locator('.inline-runner-logs')).not.toBeVisible();
  });

  test('only auto-runs the inline function while the Run tab is unmounted', async ({ page }) => {
    await page.getByRole('button', { name: 'Generated IR' }).click();
    await page.locator('.code-textarea').fill(UNRELATED_EXPORT_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );

    const codeLens = page.locator('.codelens-decoration:has-text("▶ Run inspect")');
    await expect(codeLens).toBeVisible({ timeout: 10_000 });
    await codeLens.click();

    const widget = page.locator('.inline-runner-widget');
    await expect(widget).toBeVisible();
    await expect(widget.locator('.inline-runner-name')).toHaveText('inspect');
    await expect(widget.locator('.result-value.success')).toHaveText('0');
  });
});
