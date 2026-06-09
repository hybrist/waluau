import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

const DOM_SAMPLE = `local window = require("dom:window")
local document: Document = window.document
local output_body: HTMLElement = document.body

local title: Element = document:create_element("h2")
title.text_content = "Hello from Waluau DOM"
output_body:append_child(title)

local body: Element = document:create_element("p")
body.text_content = title.text_content .. " rendered inside the playground Run tab"
output_body:append_child(body)
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
new_child.class_name = "panel ready selected"
new_child:set_attribute("data-state", "ready")
body:replace_child(new_child, old_child)

local state: string? = new_child:get_attribute("data-state")
storage:set_item("waluau-playground-dom-host-api", "persisted")
local saved: string? = storage:get_item("waluau-playground-dom-host-api")

local input_element: Element = document:create_element("input")
if HTMLInputElement(input) = input_element then
    input.value = "typed card"
    if state ~= nil then
        if saved ~= nil then
            new_child.text_content = input.value .. " " .. state .. " " .. saved
        end
    end
end

body:remove_child(new_child)
body:append_child(new_child)
`;

const DOM_EVENT_CALLBACK_SAMPLE = `local window = require("dom:window")
local document = window.document
local body: HTMLElement = document.body

local status: Element = document:create_element("p")
status.id = "event-status"
status.text_content = "idle"

local button: Element = document:create_element("button")
button.id = "event-button"
button.text_content = "Click"

local click_count: i32 = 0
button:add_event_listener("click", function(event: Event): unit
    click_count = click_count + 1
    if Element(target) = event.target then
        if click_count == 1 then
            status.text_content = "clicked " .. target.id
        else
            status.text_content = "clicked twice " .. target.id
        end
    end
end)

local input: Element = document:create_element("input")
input.id = "event-input"
input:add_event_listener("input", function(event: Event): unit
    if HTMLInputElement(target) = event.target then
        status.text_content = "input " .. target.value
    end
end)

body:append_child(button)
body:append_child(input)
body:append_child(status)
`;

const DOM_FETCH_RESPONSE_TEXT_SAMPLE = `function fetch_body(): unit
    local co: thread = coroutine.create(function(): i32
        local res = fetch("/test.json"):await()
        local response_body = res:text():await()

        local window = require("dom:window")
        local document: Document = window.document
        local body: HTMLElement = document.body
        local output: Element = document:create_element("p")
        output.id = "fetch-body"
        output.text_content = response_body
        body:append_child(output)

        return 0
    end)
    coroutine.resume(co)
end
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

  test('runs Waluau click and input callbacks from DOM Output events', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_EVENT_CALLBACK_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('#event-status')).toHaveText('idle');

    await outputFrame.locator('#event-button').click();
    await expect(outputFrame.locator('#event-status')).toHaveText('clicked event-button');

    await outputFrame.locator('#event-button').click();
    await expect(outputFrame.locator('#event-status')).toHaveText('clicked twice event-button');

    await outputFrame.locator('#event-input').fill('typed card');
    await expect(outputFrame.locator('#event-status')).toHaveText('input typed card');
  });

  test('runs Waluau fetch and Response.text awaits in DOM Output', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_FETCH_RESPONSE_TEXT_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('#fetch-body')).toHaveText(/^\{"message":"fetch body from playground"\}\s*$/);
  });

  test('supports top-level fetch and await without a manual coroutine wrapper', async ({ page }) => {
    await page.getByRole('button', { name: 'Top Level Fetch (Test)' }).click();

    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    // All three print statements must appear in the init-logs box
    await expect(page.locator('.init-logs-value')).toHaveText(
      'before fetch\nafter fetch\nafter dom update',
    );

    // The fetch body should be written to document.body.text_content
    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('body')).toHaveText(/\{"message":"fetch body from playground"\}/);
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

  test('supports fullscreen mode and escaping back to normal', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    // Verify fullscreen button is present
    const fullscreenBtn = domOutput.locator('.dom-output-fullscreen-btn');
    await expect(fullscreenBtn).toBeVisible();
    await expect(fullscreenBtn).toHaveText('Full Screen');

    // Click fullscreen button
    await fullscreenBtn.click();

    // The section should now have the "fullscreen" class
    await expect(domOutput).toHaveClass(/fullscreen/);

    // The close button/bar should be visible
    const exitBtn = domOutput.locator('.dom-output-exit-btn');
    await expect(exitBtn).toBeVisible();
    await expect(exitBtn).toHaveText('Close Full Screen');

    // Clicking close button exits fullscreen
    await exitBtn.click();
    await expect(domOutput).not.toHaveClass(/fullscreen/);
    await expect(exitBtn).not.toBeVisible();

    // Click fullscreen button again to test Escape key from main window
    await fullscreenBtn.click();
    await expect(domOutput).toHaveClass(/fullscreen/);
    await page.keyboard.press('Escape');
    await expect(domOutput).not.toHaveClass(/fullscreen/);

    // Click fullscreen button again to test Escape key from iframe
    await fullscreenBtn.click();
    await expect(domOutput).toHaveClass(/fullscreen/);
    
    // Focus inside iframe and press Escape
    const frame = page.frameLocator('.dom-output-frame');
    await frame.locator('body').click(); // focus frame
    await page.keyboard.press('Escape');
    await expect(domOutput).not.toHaveClass(/fullscreen/);
  });
});
