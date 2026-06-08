import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

async function openKanbanPreset(page) {
  await page.goto('/');
  await expect(page.locator('.status-text')).not.toHaveText('Loading Compiler...', {
    timeout: COMPILER_READY_TIMEOUT,
  });
  await page.evaluate(() => {
    for (let index = localStorage.length - 1; index >= 0; index -= 1) {
      const key = localStorage.key(index);
      if (key?.startsWith('waluau-kanban:')) {
        localStorage.removeItem(key);
      }
    }
  });
  await page.getByRole('button', { name: 'Kanban Board', exact: true }).click();
  await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
    timeout: COMPILER_READY_TIMEOUT,
  });
}

async function hasStoredCardTitle(page, title) {
  return page.evaluate((expectedTitle) => {
    for (let index = 0; index < localStorage.length; index += 1) {
      const key = localStorage.key(index);
      if (key?.startsWith('waluau-kanban:card:') && localStorage.getItem(key) === expectedTitle) {
        return true;
      }
    }
    return false;
  }, title);
}

test.describe('Kanban playground preset', () => {
  test('loads, renders, styles, edits, moves, filters, persists, and deletes cards', async ({ page }) => {
    await openKanbanPreset(page);

    await expect(page.locator('.file-item').getByText('app.walu', { exact: true })).toBeVisible();
    await expect(page.locator('.code-textarea')).toContainText('require("dom:window")');
    await expect(page.locator('.code-textarea')).not.toContainText('marshal');
    await expect(page.locator('.code-textarea')).not.toContainText('random_uuid');

    const frame = page.frameLocator('.dom-output-frame');
    await expect(frame.getByRole('heading', { name: 'Waluau Kanban' })).toBeVisible();
    for (const column of ['Backlog', 'Doing', 'Done']) {
      await expect(frame.getByRole('heading', { name: column })).toBeVisible();
    }

    await expect(frame.locator('[data-column="backlog"]')).toContainText('Persist board state');
    await expect(frame.locator('[data-column="doing"]')).toContainText('Expose DOM callbacks');
    await expect(frame.locator('[data-column="done"]')).toContainText('Ship Kanban preset');

    await expect(
      frame.locator('#app > div').evaluate((node) => getComputedStyle(node).backgroundColor),
    ).resolves.toBe('rgb(241, 245, 249)');

    await frame.getByRole('button', { name: 'Add card' }).click();
    await frame.locator('#kanban-edit-title').fill('Implement browser Kanban');
    await frame.locator('#kanban-edit-description').fill('Exercise client state from Waluau');
    await frame.locator('#kanban-edit-priority').fill('Critical');
    await frame.locator('#kanban-edit-tags').fill('waluau ui');
    await frame.getByRole('button', { name: 'Save' }).click();

    await expect(frame.locator('[data-column="backlog"]')).toContainText('Implement browser Kanban');
    await expect(frame.locator('#kanban-activity')).toContainText('Updated Implement browser Kanban');
    await expect.poll(() => hasStoredCardTitle(page, 'Implement browser Kanban')).toBe(true);

    const newCard = frame.locator('.kanban-card', { hasText: 'Implement browser Kanban' });
    await frame.locator('#kanban-filter').fill('waluau');
    await expect(frame.locator('.kanban-card')).toHaveCount(1);
    await expect(frame.locator('.kanban-card')).toContainText('Implement browser Kanban');
    await expect(frame.locator('[data-column="doing"]')).not.toContainText('Expose DOM callbacks');
    await frame.locator('#kanban-filter').fill('');

    await newCard.getByRole('button', { name: 'Edit' }).click();
    await frame.locator('#kanban-edit-title').fill('Implement edited browser Kanban');
    await frame.locator('#kanban-edit-description').fill('Exercise edited client state from Waluau');
    await frame.locator('#kanban-edit-priority').fill('Critical');
    await frame.locator('#kanban-edit-tags').fill('waluau ui');
    await frame.getByRole('button', { name: 'Save' }).click();
    const editedCard = frame.locator('.kanban-card', { hasText: 'Implement edited browser Kanban' });
    await expect(editedCard).toContainText('Exercise edited client state from Waluau');
    await expect.poll(() => hasStoredCardTitle(page, 'Implement edited browser Kanban')).toBe(true);

    await editedCard.getByRole('button', { name: 'Next' }).click();
    await expect(frame.locator('[data-column="doing"]')).toContainText('Implement edited browser Kanban');
    await expect(frame.locator('[data-column="backlog"]')).not.toContainText('Implement edited browser Kanban');

    const source = await page.locator('.code-textarea').inputValue();
    await page.locator('.code-textarea').fill(`${source}\n-- Playwright rerun marker\n`);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await expect(frame.locator('[data-column="doing"]')).toContainText('Implement edited browser Kanban');

    await page.reload();
    await expect(page.locator('.status-text')).not.toHaveText('Loading Compiler...', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await page.getByRole('button', { name: 'Kanban Board', exact: true }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await expect(frame.locator('[data-column="doing"]')).toContainText('Implement edited browser Kanban');
    await expect(frame.locator('[data-column="doing"]')).toContainText('Critical priority');

    await frame.locator('.kanban-card', { hasText: 'Implement edited browser Kanban' }).getByRole('button', { name: 'Delete' }).click();
    await expect(frame.locator('#kanban-board')).not.toContainText('Implement edited browser Kanban');
    await expect(frame.locator('#kanban-activity')).toContainText('Deleted Implement edited browser Kanban');
  });
});
