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

const DOM_KEYBOARD_EVENT_SAMPLE = `local window = require("dom:window")
local document = window.document
local body: HTMLElement = document.body

local status: Element = document:create_element("p")
status.id = "key-status"
status.text_content = "idle"
body:append_child(status)

document:add_event_listener("keydown", function(event: Event): unit
    if KeyboardEvent(kev) = event then
        status.text_content = "key " .. kev.key .. " code " .. kev.code
    end
end)
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

const DOM_CANVAS_2D_SAMPLE = `local window = require("dom:window")
local document: Document = window.document
local body: HTMLElement = document.body

local element: Element = document:create_element("canvas")
element.id = "waluau-canvas-2d"
body:append_child(element)

if HTMLCanvasElement(canvas) = element then
    canvas.width = 64
    canvas.height = 32
    local style: CSSStyleProperties = canvas.style
    style:setProperty("width", "64px")
    style:setProperty("height", "32px")

    local context: CanvasRenderingContext2D? = canvas:get_context("2d")
    assert(context ~= nil)
    local ctx: CanvasRenderingContext2D = context::CanvasRenderingContext2D
    local context_canvas: HTMLCanvasElement = ctx.canvas
    context_canvas:set_attribute("data-context-owner", "true")

    ctx:clearRect(0.0, 0.0, 64.0, 32.0)
    ctx:fillRect(4.0, 5.0, 20.0, 12.0)
    ctx.font = "10px sans-serif"
    ctx:fillText("2D", 28.0, 16.0)
else
    assert(false)
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

  test('loads DOM modules when WebAssembly.Module.imports reflection fails', async ({ page }) => {
    await page.evaluate(() => {
      WebAssembly.Module.imports = () => {
        throw new TypeError(
          'WebAssembly.Module.imports unable to produce import descriptors for the given module',
        );
      };
    });

    await page.locator('.code-textarea').fill(DOM_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('h2')).toHaveText('Hello from Waluau DOM');
    await expect(page.getByRole('heading', { name: 'Module Load Error' })).toHaveCount(0);
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

  test('renders generated canvas 2D output in the playground DOM frame', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_CANVAS_2D_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    const outputFrame = page.frameLocator('.dom-output-frame');
    const canvas = outputFrame.locator('canvas#waluau-canvas-2d');
    await expect(canvas).toHaveAttribute('data-context-owner', 'true');
    await expect(canvas).toHaveJSProperty('width', 64);
    await expect(canvas).toHaveJSProperty('height', 32);

    const rect = await canvas.evaluate((node) => {
      const bounds = node.getBoundingClientRect();
      return { width: bounds.width, height: bounds.height };
    });
    expect(rect).toEqual({ width: 64, height: 32 });

    const pixel = await canvas.evaluate((node) =>
      Array.from(node.getContext('2d').getImageData(6, 7, 1, 1).data)
    );
    expect(pixel).toEqual([0, 0, 0, 255]);
  });

  test('runs the snake fixture through the 2D game engine', async ({ page }) => {
    const pageErrors = [];
    page.on('pageerror', (error) => pageErrors.push(error.message));
    await page.getByRole('button', { name: 'Snake Game' }).click();

    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('h1')).toHaveText('Waluau Snake');

    const canvas = outputFrame.locator('canvas#walua-game-canvas');
    await expect.poll(() =>
      canvas.evaluate((node) => ({
        densityMatches:
          node.width === Math.round(node.clientWidth * window.devicePixelRatio) &&
          node.height === Math.round(node.clientHeight * window.devicePixelRatio),
        aspectMatches: Math.abs(node.clientWidth / node.clientHeight - 1) < 0.01,
      })),
    ).toEqual({ densityMatches: true, aspectMatches: true });

    const signature = () =>
      canvas.evaluate((node) => {
        const gl = node.getContext('webgl2');
        const data = new Uint8Array(node.width * node.height * 4);
        gl.readPixels(0, 0, node.width, node.height, gl.RGBA, gl.UNSIGNED_BYTE, data);
        let hash = 0;
        for (let i = 0; i < data.length; i += 16) {
          hash = (hash * 33 + data[i] + data[i + 1] * 3 + data[i + 2] * 7) >>> 0;
        }
        return hash;
      });

    const initial = await signature();
    await expect.poll(signature).not.toBe(initial);
    await canvas.click();
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('s');
    await page.keyboard.press('r');
    await expect.poll(signature).not.toBe(initial);
    expect(pageErrors).toEqual([]);
  });

  test('runs the 2D game engine preset through its platform facade', async ({ page }) => {
    await page.getByRole('button', { name: '2D Game Engine' }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('h1')).toHaveText('Walua 2D Engine (WebGL2)');
    const canvas = outputFrame.locator('canvas#walua-game-canvas');
    await expect.poll(() =>
      canvas.evaluate((node) => ({
        densityMatches:
          node.width === Math.round(node.clientWidth * window.devicePixelRatio) &&
          node.height === Math.round(node.clientHeight * window.devicePixelRatio),
        aspectMatches: Math.abs(node.clientWidth / node.clientHeight - 1.6) < 0.01,
      })),
    ).toEqual({ densityMatches: true, aspectMatches: true });

    // The engine renders through WebGL2 (acquired with preserveDrawingBuffer),
    // so the frame signature reads back through gl.readPixels.
    const signature = () =>
      canvas.evaluate((node) => {
        const gl = node.getContext('webgl2');
        const data = new Uint8Array(node.width * node.height * 4);
        gl.readPixels(0, 0, node.width, node.height, gl.RGBA, gl.UNSIGNED_BYTE, data);
        let hash = 0;
        for (let i = 0; i < data.length; i += 16) {
          hash = (hash * 33 + data[i] + data[i + 1] * 3 + data[i + 2] * 7) >>> 0;
        }
        return hash;
      });

    const initial = await signature();
    await expect.poll(signature).not.toBe(initial);
    await canvas.click();
    await page.keyboard.press('ArrowLeft');
    await page.keyboard.press('r');
    await expect.poll(signature).not.toBe(initial);
  });

  test('runs the particle system demos preset and switches scenes', async ({ page }) => {
    const pageErrors = [];
    page.on('pageerror', (error) => pageErrors.push(error.message));
    await page.getByRole('button', { name: 'Particle System Demos' }).click();
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('h1')).toHaveText('Waluau Particle System Demos');
    const canvas = outputFrame.locator('canvas#walua-game-canvas');

    const signature = () =>
      canvas.evaluate((node) => {
        const gl = node.getContext('webgl2');
        const data = new Uint8Array(node.width * node.height * 4);
        gl.readPixels(0, 0, node.width, node.height, gl.RGBA, gl.UNSIGNED_BYTE, data);
        let hash = 0;
        for (let i = 0; i < data.length; i += 16) {
          hash = (hash * 33 + data[i] + data[i + 1] * 3 + data[i + 2] * 7) >>> 0;
        }
        return hash;
      });

    // Particles animate continuously, so the frame signature keeps moving.
    const initial = await signature();
    await expect.poll(signature).not.toBe(initial);

    // Walk through every scene (fire, fountain, fireworks, snow, galaxy,
    // areas); each keeps rendering and none may throw.
    await canvas.click();
    for (let scene = 0; scene < 6; scene += 1) {
      await page.keyboard.press('ArrowRight');
      const before = await signature();
      await expect.poll(signature).not.toBe(before);
    }
    expect(pageErrors).toEqual([]);
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

  test('reads key and code from KeyboardEvent downcasts in keydown listeners', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_KEYBOARD_EVENT_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('#key-status')).toHaveText('idle');

    // Move focus into the DOM Output iframe so key presses reach its document.
    await outputFrame.locator('body').click();
    await page.keyboard.press('a');
    await expect(outputFrame.locator('#key-status')).toHaveText('key a code KeyA');

    await page.keyboard.press('ArrowLeft');
    await expect(outputFrame.locator('#key-status')).toHaveText('key ArrowLeft code ArrowLeft');
  });

  test('runs Waluau fetch and Response.text awaits in DOM Output', async ({ page }) => {
    let fetchRequests = 0;
    await page.route('**/test.json', async (route) => {
      fetchRequests += 1;
      await new Promise((resolve) => setTimeout(resolve, 250));
      await route.fulfill({
        contentType: 'application/json',
        body: '{"message":"fetch body from playground"}',
      });
    });

    await page.locator('.code-textarea').fill(DOM_FETCH_RESPONSE_TEXT_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();
    await expect.poll(() => fetchRequests).toBe(1);

    // Fullscreen state changes rerender RunTab while the fetch coroutine is
    // suspended. Rendering the result must not execute the export again.
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      domOutput.locator('.dom-output-fullscreen-btn').click(),
    ]);
    await expect(page).toHaveURL(/fullscreen=true/);
    
    // Close the popup/exit fullscreen
    await domOutput.locator('.dom-output-fullscreen-btn').click();
    await expect(popup.isClosed()).toBe(true);
    await expect(page).not.toHaveURL(/fullscreen=true/);

    const outputFrame = page.frameLocator('.dom-output-frame');
    await expect(outputFrame.locator('#fetch-body')).toHaveCount(1);
    await expect(outputFrame.locator('#fetch-body')).toHaveText(/^\{"message":"fetch body from playground"\}\s*$/);
    
    // Since switching domOutputRoot between iframe and popup re-runs the compiler/wasm module,
    // fetchRequests will be incremented. We expect 3 requests (initial run, popup open run, popup close run).
    await expect.poll(() => fetchRequests).toBe(3);
  });

  test('supports top-level fetch and await without a manual coroutine wrapper', async ({ page }) => {
    await page.getByRole('button', { name: 'Top Level Fetch (Test)' }).click();

    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    // All three print statements must appear in the init-logs box.
    // Allow extra time: the last two arrive after the async fetch resolves.
    await expect(page.locator('.init-logs-value')).toHaveText(
      'before fetch\nafter fetch\nafter dom update',
      { timeout: COMPILER_READY_TIMEOUT },
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

  test('supports fullscreen mode and closing it', async ({ page }) => {
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

    // Click fullscreen button and wait for the popup to open
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      fullscreenBtn.click(),
    ]);

    // Verify popup is open and URL parameter is set
    await expect(popup).toBeDefined();
    await expect(page.url()).toContain('fullscreen=true');

    // Verify placeholder is shown on the main page
    const placeholder = domOutput.locator('.dom-output-placeholder');
    await expect(placeholder).toBeVisible();
    await expect(placeholder.locator('.dom-output-placeholder-text')).toHaveText(
      'Preview active in a separate fullscreen window'
    );

    // Verify popup content
    await expect(popup.locator('h2')).toHaveText('Hello from Waluau DOM');

    // Clicking Exit Full Screen button on the main page closes the popup
    const exitBtn = domOutput.locator('.dom-output-fullscreen-btn');
    await expect(exitBtn).toHaveText('Exit Full Screen');
    await exitBtn.click();

    // Verify popup is closed and URL updated
    await expect(popup.isClosed()).toBe(true);
    await expect(page.url()).not.toContain('fullscreen=true');
    await expect(placeholder).not.toBeVisible();
    await expect(domOutput.locator('.dom-output-frame')).toBeVisible();
  });

  test('supports closing the popup window directly', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    const fullscreenBtn = domOutput.locator('.dom-output-fullscreen-btn');

    // Click fullscreen button and wait for the popup
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      fullscreenBtn.click(),
    ]);

    await expect(page).toHaveURL(/fullscreen=true/);

    // Close the popup window directly
    await popup.close();

    // Verify main page updates state (url updated, placeholder removed)
    await expect(page).not.toHaveURL(/fullscreen=true/);
    await expect(domOutput.locator('.dom-output-placeholder')).not.toBeVisible();
    await expect(domOutput.locator('.dom-output-frame')).toBeVisible();
  });

  test('updates the popup content dynamically as the code is edited', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    const fullscreenBtn = domOutput.locator('.dom-output-fullscreen-btn');

    // Open popup
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      fullscreenBtn.click(),
    ]);

    await expect(popup.locator('h2')).toHaveText('Hello from Waluau DOM');

    // Edit code in editor
    const MODIFIED_DOM_SAMPLE = DOM_SAMPLE.replace(
      'Hello from Waluau DOM',
      'Hello from updated Waluau DOM'
    );
    await page.locator('.code-textarea').fill(MODIFIED_DOM_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    // Verify popup content updates
    await expect(popup.locator('h2')).toHaveText('Hello from updated Waluau DOM');

    // Clean up
    await popup.close();
  });

  test('starts directly in fullscreen mode if URL parameter is present', async ({ page }) => {
    // Navigate with fullscreen=true parameter
    await page.goto('/?fullscreen=true');

    // Fill the editor with DOM_SAMPLE to trigger the popup opening
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      page.locator('.code-textarea').fill(DOM_SAMPLE),
    ]);

    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    await expect(domOutput).toBeVisible();

    // Verify placeholder is shown on the main page
    await expect(domOutput.locator('.dom-output-placeholder')).toBeVisible();

    // Verify content inside popup
    await expect(popup.locator('h2')).toHaveText('Hello from Waluau DOM');

    // Exiting fullscreen should close the popup and update URL
    const exitBtn = domOutput.locator('.dom-output-fullscreen-btn');
    await exitBtn.click();
    await expect(popup.isClosed()).toBe(true);
    await expect(page).not.toHaveURL(/fullscreen=true/);
  });

  test('automatically moves focus to the popup when entering fullscreen mode', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    const fullscreenBtn = domOutput.locator('.dom-output-fullscreen-btn');

    // Click fullscreen button
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      fullscreenBtn.click(),
    ]);

    // Check popup document has focus
    await expect.poll(async () => {
      return await popup.evaluate(() => document.hasFocus());
    }).toBe(true);

    await popup.close();
  });

  test('automatically moves focus to the popup when starting directly in fullscreen mode', async ({ page }) => {
    // Navigate with fullscreen=true parameter
    await page.goto('/?fullscreen=true');

    // Fill the editor with DOM_SAMPLE to trigger the popup opening
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      page.locator('.code-textarea').fill(DOM_SAMPLE),
    ]);

    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    // Check popup document has focus
    await expect.poll(async () => {
      return await popup.evaluate(() => document.hasFocus());
    }).toBe(true);

    await popup.close();
  });

  test('automatically moves focus to the popup when compiling while already in fullscreen mode', async ({ page }) => {
    await page.locator('.code-textarea').fill(DOM_SAMPLE);
    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    const domOutput = page.getByLabel('DOM Output');
    const fullscreenBtn = domOutput.locator('.dom-output-fullscreen-btn');
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      fullscreenBtn.click(),
    ]);

    // Check popup document has focus
    await expect.poll(async () => {
      return await popup.evaluate(() => document.hasFocus());
    }).toBe(true);

    // Focus somewhere else to lose focus
    await page.getByRole('button', { name: 'REPL' }).focus();

    // Fill the textarea with new source code to trigger re-compilation/run
    await page.locator('.code-textarea').fill(DOM_SAMPLE + '\n-- rerun focus test');
    // Shift focus immediately to a button so the compiler run sees that we are not typing
    await page.getByRole('button', { name: 'REPL' }).focus();

    await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
      timeout: COMPILER_READY_TIMEOUT,
    });

    // Check popup document is focused again
    await expect.poll(async () => {
      return await popup.evaluate(() => document.hasFocus());
    }).toBe(true);

    await popup.close();
  });
});
