import { test, expect } from '@playwright/test';

test.describe('output rendering route', () => {
  test('directly renders poker tricks without playground UI', async ({ page }) => {
    // Navigate directly to the output route for poker tricks
    await page.goto('/output/poker-tricks');

    // Wait until the canvas is mounted directly on the body
    const canvas = page.locator('canvas#walua-game-canvas');
    await expect(canvas).toBeVisible({ timeout: 20000 });

    // Verify it is directly under #walua-game in the body (no iframe)
    const gameContainer = page.locator('#walua-game');
    await expect(gameContainer).toBeVisible();

    // Verify the playground UI elements are NOT visible
    await expect(page.locator('.app-header')).not.toBeVisible();
    await expect(page.locator('.playground-main')).not.toBeVisible();

    // Verify the game canvas dimensions
    await expect(canvas).toHaveJSProperty('width', 960);
    await expect(canvas).toHaveJSProperty('height', 600);
  });
});
