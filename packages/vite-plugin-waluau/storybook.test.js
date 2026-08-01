import assert from 'node:assert/strict';
import { test } from 'node:test';

import { createWaluauBook, WALUAU_STORY_VIEWPORT_ID } from './storybook.js';

/**
 * A compiled story module, faked: `run` registers stories the way
 * engine/storybook.walu's top level does, and the trampoline export calls the
 * registered closures back the way the compiler's does.
 */
function fakeModule({ stories, onShow, onTeardown }) {
  const calls = [];
  const exports = {
    __waluau_call_callback_unit: (callback) => callback(),
  };
  return {
    calls,
    run: async ({ createImports }) => {
      const host = createImports({ getWasmExports: () => exports });
      for (const name of stories) {
        host.__waluau_storybook_story(name, () => {
          calls.push(`show:${name}`);
          onShow?.(name);
        });
      }
      host.__waluau_storybook_teardown(() => {
        calls.push('teardown');
        onTeardown?.();
      });
      return { exports };
    },
  };
}

function fakeElement() {
  const children = [];
  const element = {
    id: '',
    children,
    setAttribute() {},
    appendChild(child) {
      children.push(child);
      child.parent = element;
      return child;
    },
    remove() {
      const index = element.parent?.children.indexOf(element) ?? -1;
      if (index >= 0) element.parent.children.splice(index, 1);
    },
    querySelector(selector) {
      return children.find((child) => `#${child.id}` === selector) ?? null;
    },
  };
  return element;
}

function withFakeDocument(run) {
  const previous = globalThis.document;
  globalThis.document = { createElement: () => fakeElement() };
  return Promise.resolve(run()).finally(() => {
    globalThis.document = previous;
  });
}

const bookFor = (module) => createWaluauBook({
  run: module.run,
  createImports: (context, hostImports) => hostImports,
});

test('loads the module once and mounts a published story into the canvas element', async () => {
  await withFakeDocument(async () => {
    let loads = 0;
    const module = fakeModule({ stories: ['Face up', 'Face down'] });
    const counted = { ...module, run: (options) => { loads += 1; return module.run(options); } };
    const book = createWaluauBook({
      run: counted.run,
      createImports: (context, hostImports) => hostImports,
    });
    const canvas = fakeElement();

    await book.mount('Face up', canvas);
    await book.mount('Face down', canvas);

    assert.equal(loads, 1);
    assert.deepEqual(module.calls, ['show:Face up', 'show:Face down']);
    assert.deepEqual(book.names(), ['Face up', 'Face down']);
    assert.equal(canvas.children.length, 1);
    assert.equal(canvas.children[0].id, WALUAU_STORY_VIEWPORT_ID);
  });
});

test('names the published stories when asked for one that does not exist', async () => {
  await withFakeDocument(async () => {
    const book = bookFor(fakeModule({ stories: ['Face up'] }));

    await assert.rejects(
      () => book.mount('Sideways', fakeElement()),
      /not published by this Waluau story file \(published: "Face up"\)/,
    );
  });
});

// Storybook tears the previous story down before rendering the next one. Doing
// that immediately would drop the WebGL context between two stories that share
// the canvas, so the teardown has to lose the race with the next mount.
test('keeps the session when the next story claims the book first', async () => {
  await withFakeDocument(async () => {
    const module = fakeModule({ stories: ['Face up', 'Face down'] });
    const book = bookFor(module);
    const canvas = fakeElement();

    await book.mount('Face up', canvas);
    book.unmount();
    await book.mount('Face down', canvas);
    await new Promise((resolve) => { setTimeout(resolve, 5); });

    assert.deepEqual(module.calls, ['show:Face up', 'show:Face down']);
  });
});

test('tears the session down once nothing else claims the book', async () => {
  await withFakeDocument(async () => {
    const module = fakeModule({ stories: ['Face up'] });
    const book = bookFor(module);
    const canvas = fakeElement();

    await book.mount('Face up', canvas);
    book.unmount();
    await new Promise((resolve) => { setTimeout(resolve, 5); });

    assert.deepEqual(module.calls, ['show:Face up', 'teardown']);
    assert.deepEqual(canvas.children, []);
  });
});

test('releases the old session when Storybook renders into a new element', async () => {
  await withFakeDocument(async () => {
    const module = fakeModule({ stories: ['Face up'] });
    const book = bookFor(module);

    await book.mount('Face up', fakeElement());
    await book.mount('Face up', fakeElement());

    assert.deepEqual(module.calls, ['show:Face up', 'teardown', 'show:Face up']);
  });
});
