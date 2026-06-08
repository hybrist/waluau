import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

const DOM_SAMPLE = `type Document = extern
type Element = extern

declare function dom_document(): Document
declare property Element:inner_text: string
declare function Document:create_element(tag: string): Element
declare function Document:append_child(child: Element): unit

local document: Document = dom_document()
local title: Element = document:create_element("h2")
title.inner_text = "Hello from Waluau DOM"
document:append_child(title)

local body: Element = document:create_element("p")
body.inner_text = title.inner_text .. " rendered inside the playground Run tab"
document:append_child(body)
`;

const DOM_HOST_API_SAMPLE = `local window = require("dom:window")
local document = window.document
local storage: Storage = window.local_storage
local body: HTMLElement = document.body

storage:remove_item("waluau-playground-dom-host-api")
local missing: string? = storage:get_item("waluau-playground-dom-host-api")
assert(missing == nil)

local old_child: Element = document:create_element("span")
old_child.id = "old-child"
body:append_child(old_child)

local new_child: Element = document:create_element("section")
new_child.id = "new-child"
new_child.class_name = "panel"
new_child:append_class("ready selected")
new_child:set_attribute("data-state", "ready")
body:replace_child(new_child, old_child)

local state: string? = new_child:get_attribute("data-state")
storage:set_item("waluau-playground-dom-host-api", "persisted")
local saved: string? = storage:get_item("waluau-playground-dom-host-api")

local input_element: Element = document:create_element("input")
input_element:set_attribute("value", "typed card")
if HTMLInputElement(input) = input_element then
    if state ~= nil then
        if saved ~= nil then
            new_child.text_content = input.value .. " " .. state .. " " .. saved
        end
    end
end

body:remove_child(new_child)
body:append_child(new_child)
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
    await expect(domOutput.locator('.dom-output-frame')).toBeVisible();
    await expect(domOutput.locator('h2')).toHaveCount(0);

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('h2')).toHaveText('Hello from Waluau DOM');
    await expect(outputFrame.locator('p')).toHaveText('Hello from Waluau DOM rendered inside the playground Run tab');
  });

  test('loads generated DOM APIs from the DOM preset via require("dom:window")', async ({ page }) => {
    await page.getByRole('button', { name: 'DOM Externs Example' }).click();

    await expect(page.locator('.file-item').getByText('main.walu', { exact: true })).toBeVisible();
    await expect(page.locator('.file-item').getByText('externs/dom.walu', { exact: true })).toHaveCount(0);
    await expect(page.locator('.code-textarea')).toContainText('local window = require("dom:window")');
    await expect(page.locator('.code-textarea')).toContainText('local document = window.document');
    await expect(page.locator('.code-textarea')).not.toContainText('type Document = extern');
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('h2#playground-title')).toHaveText('Hello from generated DOM externs');
    await expect(outputFrame.locator('h2#playground-title')).toHaveClass(/generated/);
    await expect(outputFrame.locator('h2#playground-title')).toHaveAttribute('data-source', 'waluau');
    await expect(outputFrame.locator('p')).toHaveText('Hello from generated DOM externs in a sandboxed output document with persisted state');
    await expect(outputFrame.locator('span#input-value')).toHaveText('typed value');
  });

  test('supports generated DOM mutation, attributes, input values, and localStorage', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_HOST_API_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('#old-child')).toHaveCount(0);
    await expect(outputFrame.locator('section#new-child')).toHaveClass(/panel ready selected/);
    await expect(outputFrame.locator('section#new-child')).toHaveAttribute('data-state', 'ready');
    await expect(outputFrame.locator('section#new-child')).toHaveText('typed card ready persisted');
  });

  test('loads DOM externs for the dom_extern_rendering conformance preset', async ({ page }) => {
    await page.getByRole('button', { name: 'Dom Extern Rendering (Test)' }).click();

    await expect(page.locator('.file-item').getByText('dom_extern_rendering.walu', { exact: true })).toBeVisible();
    await expect(page.locator('.file-item').getByText('externs/dom.walu', { exact: true })).toHaveCount(0);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('h1#generated-heading')).toHaveText('Hello from generated DOM externsleaf');
    await expect(outputFrame.locator('h1#generated-heading span.leaf')).toHaveText('leaf');
    await expect(outputFrame.locator('p#generated-paragraph')).toHaveText('Rendered through generated extern DOM handles');
  });
});
