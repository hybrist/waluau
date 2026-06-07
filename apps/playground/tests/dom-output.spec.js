import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

const DOM_SAMPLE = `type Element = extern

declare function dom_document(): Element
declare function dom_create_element(root: Element, tag: string): Element
declare function dom_set_text(element: Element, text: string): unit
declare function dom_append_child(parent: Element, child: Element): unit

local root: Element = dom_document()
local title: Element = dom_create_element(root, "h2")
dom_set_text(title, "Hello from Waluau DOM")
dom_append_child(root, title)

local body: Element = dom_create_element(root, "p")
dom_set_text(body, "Rendered inside the playground Run tab")
dom_append_child(root, body)
`;

test.describe('DOM Output in Run tab', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.status-text')).toHaveText(
      'Compilation Succeeded',
      { timeout: COMPILER_READY_TIMEOUT },
    );
    await page.getByRole('button', { name: 'Run' }).click();
  });

  test('omits DOM Output for non-DOM programs', async ({ page }) => {
    await expect(page.getByLabel('DOM Output')).toHaveCount(0);
  });

  test('renders Waluau-created DOM elements in DOM Output', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();
    await expect(domOutput.locator('h2')).toHaveText('Hello from Waluau DOM');
    await expect(domOutput.locator('p')).toHaveText('Rendered inside the playground Run tab');
  });
});
