import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('editor', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.status-text')).not.toHaveText(
      'Loading Compiler...',
      { timeout: COMPILER_READY_TIMEOUT },
    );
  });

  test('replacing editor content with valid code compiles successfully', async ({ page }) => {
    await page.locator('.code-textarea').fill(
      'function answer(): i32\n    return 42\nend',
    );
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
  });

  test('invalid code triggers a compilation failure', async ({ page }) => {
    await page.locator('.code-textarea').fill('this is not valid waluau code!!!');
    await expect(page.locator('.status-text')).toHaveText('Compilation Failed', {
      timeout: COMPILER_READY_TIMEOUT,
    });
  });

  test('compilation error message appears in the IR tab error state', async ({ page }) => {
    await page.locator('.code-textarea').fill('???');
    await expect(page.locator('.status-text')).toHaveText('Compilation Failed', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    // Switch to Generated IR tab to see the diagnostic.
    await page.getByRole('button', { name: 'Generated IR' }).click();
    await expect(page.locator('.diagnostic-output')).toBeVisible();
  });

  test('line numbers reflect the current line count', async ({ page }) => {
    const threeLineProgram = 'function f(): i32\n    return 1\nend';
    await page.locator('.code-textarea').fill(threeLineProgram);
    await expect(page.locator('.monaco-editor .line-numbers')).toHaveCount(3);
  });

  test('Monaco Editor uses the custom waluau language mode', async ({ page }) => {
    await page.waitForSelector('.monaco-editor');
    const languageId = await page.evaluate(() => {
      return window.monaco?.editor?.getModels()?.[0]?.getLanguageId();
    });
    expect(languageId).toBe('waluau');
  });

  test('current Waluau declarations and primitive types are syntax highlighted', async ({ page }) => {
    await page.waitForSelector('.monaco-editor');
    const primitiveTypes = [
      'number',
      'u32',
      'u64',
      'i32',
      'i64',
      'f32',
      'f64',
      'unit',
      'void',
      'bool',
      'unknown',
      'string',
      'bytes',
      'extern',
      'thread',
    ];
    const highlighted = await page.evaluate((types) => {
      const source = [
        'export type State = { value: i32 }',
        'export opaque type Handle = extern extends Node',
        'export enum Direction { north, south }',
        'match direction do',
        'case Direction.north then',
        'end',
        ...types.map((type, index) => `type Primitive${index} = ${type}`),
      ].join('\n');
      const lines = source.split('\n');
      return window.monaco.editor.tokenize(source, 'waluau').flatMap((tokens, lineIndex) =>
        tokens.map((token, tokenIndex) => ({
          text: lines[lineIndex].slice(
            token.offset,
            tokens[tokenIndex + 1]?.offset ?? lines[lineIndex].length,
          ).trim(),
          type: token.type,
        })),
      ).filter((token) => token.text);
    }, primitiveTypes);

    for (const keyword of ['export', 'type', 'opaque', 'extends', 'enum', 'match', 'case']) {
      expect(highlighted).toContainEqual({ text: keyword, type: 'keyword.waluau' });
    }
    for (const type of primitiveTypes) {
      expect(highlighted).toContainEqual({ text: type, type: 'type.waluau' });
    }
  });

  test('go to definition opens an imported enum in its file', async ({ page }) => {
    await page.getByRole('button', { name: 'Require Flow Example' }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    await page.locator('.file-tab').filter({ hasText: 'ops.walu' }).click();
    await page.locator('.code-textarea').fill('export enum E { A }');
    await page.locator('.file-tab').filter({ hasText: 'namespace_main.walu' }).click();
    await page.locator('.code-textarea').fill(
      'local e_lib = require("./ops")\n\nlocal e = e_lib.E.A',
    );
    // The status flips through 'Analyzing...' synchronously on each edit and
    // can settle back to success between expect polls, so only wait for the
    // final state here.
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    // The definition provider answers from the worker, which may still be
    // analyzing the freshly filled files, so a first ctrl+click can be a
    // no-op. Retry the whole gesture until the tab actually switches.
    await expect(async () => {
      const activeTab = page.locator('.file-tab.active .file-name-text');
      if ((await activeTab.textContent()) === 'ops.walu') return;

      const variant = await page
        .locator('.monaco-editor .view-line')
        .filter({ hasText: 'local e = e_lib.E.A' })
        .evaluate((line) => {
          const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
          for (let text = walker.nextNode(); text; text = walker.nextNode()) {
            const index = text.textContent.lastIndexOf('A');
            if (index < 0) continue;
            const range = document.createRange();
            range.setStart(text, index);
            range.setEnd(text, index + 1);
            const bounds = range.getBoundingClientRect();
            return { x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 };
          }
          throw new Error('enum variant A was not rendered');
        });
      await page.keyboard.down('Control');
      try {
        await page.mouse.click(variant.x, variant.y);
      } finally {
        await page.keyboard.up('Control');
      }

      await expect(activeTab).toHaveText('ops.walu', { timeout: 2_000 });
    }).toPass({ timeout: COMPILER_READY_TIMEOUT });
    await expect(page.locator('.code-textarea')).toHaveValue('export enum E { A }');
    await expect(page.locator('.monaco-editor .view-line').first()).toContainText(
      'export enum E { A }',
    );
  });

  test('entry-point error message appears in the run tab when top-level code traps', async ({ page }) => {
    await page.locator('.code-textarea').fill('assert(false)');
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await expect(page.getByRole('heading', { name: 'Module Load Error' })).toBeVisible();
    await expect(page.locator('.diagnostic-output')).toContainText(
      'Failed to execute the generated WASM module entry point:',
    );
    await expect(page.locator('.diagnostic-output')).not.toContainText('This module requires Wasm GC');
  });

  test('generated Wasm compile failures show the browser diagnostic in the run tab', async ({ page }) => {
    await page.evaluate(() => {
      const originalCompile = WebAssembly.compile;
      WebAssembly.compile = async (...args) => {
        WebAssembly.compile = originalCompile;
        void args;
        throw new WebAssembly.CompileError(
          'simulated mobile Safari rejection at byte 42: ref.cast failed',
        );
      };
    });

    await page.locator('.code-textarea').fill(
      'function identity(values: {i32}): {i32}\n    return values\nend',
    );
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await expect(page.getByRole('heading', { name: 'Module Load Error' })).toBeVisible();
    await expect(page.locator('.diagnostic-output')).toContainText(
      'Failed to compile the generated WASM module: CompileError: simulated mobile Safari rejection at byte 42: ref.cast failed',
    );
    await expect(page.locator('.diagnostic-output')).toContainText(
      'This module uses Wasm GC.',
    );
  });
});
