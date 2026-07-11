import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

async function evaluate(page, source) {
  const input = page.locator('.repl-input');
  await input.click();
  await input.fill(source);
  await page.locator('.repl-run-btn').click();
}

test.describe('REPL tab', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Wait for the compiler to be ready before driving the REPL.
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await page.getByRole('button', { name: 'REPL' }).click();
    // Opening the tab auto-seeds from the editor (the default "Add" preset has
    // no top-level output, so it is a single acknowledged cell). Clear it so
    // each behavior test starts from a deterministic blank session.
    await expect(page.locator('.repl-cell')).toHaveCount(1);
    await page.getByRole('button', { name: 'Reset session' }).click();
    await expect(page.locator('.repl-cell')).toHaveCount(0);
  });

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

  test('reports generated Wasm load errors with the browser diagnostic', async ({ page }) => {
    await page.evaluate(() => {
      const originalCompile = WebAssembly.compile;
      WebAssembly.compile = async (...args) => {
        WebAssembly.compile = originalCompile;
        void args;
        throw new WebAssembly.CompileError('simulated Safari REPL module rejection');
      };
    });

    await evaluate(page, 'print("will not instantiate")');
    await expect(page.locator('.repl-output-error').last()).toContainText(
      'Failed to compile the generated WASM module: CompileError: simulated Safari REPL module rejection',
    );

    // The failed module did not commit the cell, and restoring the browser API
    // lets the next evaluation proceed normally.
    await evaluate(page, 'print("recovered")');
    await expect(page.locator('.repl-cell-output').last()).toHaveText('recovered');
  });

  test('reset clears the session transcript', async ({ page }) => {
    await evaluate(page, 'print("before reset")');
    await expect(page.locator('.repl-cell')).toHaveCount(1);
    await page.getByRole('button', { name: 'Reset session' }).click();
    await expect(page.locator('.repl-cell')).toHaveCount(0);
  });
});

test.describe('REPL seeding from the editor script', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    // The top-level-statements fixture prints at module init and defines a
    // function, so it exercises both seeded output and seeded definitions.
    await page.getByRole('button', { name: 'Top_level_statements' }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await page.getByRole('button', { name: 'REPL' }).click();
  });

  test('auto-seeds with the script output on first open', async ({ page }) => {
    const seedCell = page.locator('.repl-cell-seed');
    await expect(seedCell).toHaveCount(1);
    await expect(seedCell).toContainText('loaded editor script (top_level_statements.walu)');
    // Top-level prints from the script appear as the seed cell's output.
    await expect(seedCell.locator('.repl-cell-output')).toContainText('Init statement run');
    await expect(seedCell.locator('.repl-cell-output')).toContainText('Function called');
  });

  test('cells can call functions defined by the seeded script', async ({ page }) => {
    await expect(page.locator('.repl-cell-seed')).toHaveCount(1);
    // `answer` is defined in the seeded script and prints when called.
    await evaluate(page, 'local result: i32 = answer()');
    await expect(page.locator('.repl-cell:not(.repl-cell-seed) .repl-cell-output').last())
      .toHaveText('Function called');
  });

  test('Load editor script re-seeds the session', async ({ page }) => {
    await evaluate(page, 'print("scratch")');
    await expect(page.locator('.repl-cell')).toHaveCount(2);
    await page.getByRole('button', { name: 'Load editor script' }).click();
    // Re-seeding restarts the session: just the single seed cell remains.
    await expect(page.locator('.repl-cell')).toHaveCount(1);
    await expect(page.locator('.repl-cell-seed')).toHaveCount(1);
  });
});
