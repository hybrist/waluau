import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('REPL tab', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Wait for the compiler to be ready before driving the REPL.
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await page.getByRole('button', { name: 'REPL' }).click();
  });

  async function evaluate(page, source) {
    const input = page.locator('.repl-input');
    await input.click();
    await input.fill(source);
    await page.locator('.repl-run-btn').click();
  }

  test('runs a print statement and shows its output', async ({ page }) => {
    await evaluate(page, 'print("hello from the REPL")');
    await expect(page.locator('.repl-cell-output').last()).toHaveText('hello from the REPL');
  });

  test('accumulates state across cells', async ({ page }) => {
    await evaluate(page, 'local name = "world"');
    // A definition with no print is acknowledged with "ok".
    await expect(page.locator('.repl-cell-ok').last()).toBeVisible();

    await evaluate(page, 'print("hello " .. name)');
    await expect(page.locator('.repl-cell-output').last()).toHaveText('hello world');
  });

  test('shows only the new output, not prior cells', async ({ page }) => {
    await evaluate(page, 'print("first")');
    await evaluate(page, 'print("second")');
    const outputs = page.locator('.repl-cell-output');
    await expect(outputs).toHaveCount(2);
    await expect(outputs.nth(0)).toHaveText('first');
    await expect(outputs.nth(1)).toHaveText('second');
  });

  test('reports a compile error without committing the cell', async ({ page }) => {
    await evaluate(page, 'this is not valid waluau');
    await expect(page.locator('.repl-cell-error').last()).toBeVisible();

    // The bad cell did not pollute the session: a fresh statement still works.
    await evaluate(page, 'print("still works")');
    await expect(page.locator('.repl-cell-output').last()).toHaveText('still works');
  });

  test('reset clears the session transcript', async ({ page }) => {
    await evaluate(page, 'print("before reset")');
    await expect(page.locator('.repl-cell')).toHaveCount(1);
    await page.getByRole('button', { name: 'Reset session' }).click();
    await expect(page.locator('.repl-cell')).toHaveCount(0);
  });
});
