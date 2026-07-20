import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  captureWaluauGame,
  createWaluauHotHost,
  HotReplacementFallback,
  replaceWaluauGame,
} from './hot.js';

test('captures, disposes, and restores compatible game state', async () => {
  const calls = [];
  const previous = captureWaluauGame(Promise.resolve({
    hotReplacement: {
      isRegistered: () => true,
      capture() {
        calls.push('snapshot');
        return '{"version":1,"score":7}';
      },
      dispose() {
        calls.push('dispose-old');
      },
    },
  }));
  const replacement = {
    hotReplacement: {
      isRegistered: () => true,
      restore(snapshot) {
        calls.push(['restore', snapshot]);
        return true;
      },
      dispose() {
        calls.push('dispose-new');
      },
    },
  };
  const reloads = [];

  const loaded = await replaceWaluauGame({
    previous,
    start: async () => {
      calls.push('start');
      return replacement;
    },
    reload: (reason) => reloads.push(reason),
  });

  assert.equal(loaded, replacement);
  assert.deepEqual(calls, [
    'snapshot',
    'dispose-old',
    'start',
    ['restore', '{"version":1,"score":7}'],
  ]);
  assert.deepEqual(reloads, []);
});

test('falls back without starting a replacement when capture is unsupported', async () => {
  const previous = captureWaluauGame(Promise.resolve({}));
  let started = false;
  const reloads = [];

  await assert.rejects(
    replaceWaluauGame({
      previous,
      start: async () => {
        started = true;
        return { exports: {} };
      },
      reload: (reason) => reloads.push(reason),
    }),
    HotReplacementFallback,
  );

  assert.equal(started, false);
  assert.equal(reloads.length, 1);
  assert.match(reloads[0], /did not register/);
});

test('disposes the replacement and reloads when its snapshot is incompatible', async () => {
  const previous = Promise.resolve({ ok: true, snapshot: 'schema:1' });
  const calls = [];
  const reloads = [];

  await assert.rejects(
    replaceWaluauGame({
      previous,
      start: async () => ({
        hotReplacement: {
          isRegistered: () => true,
          restore(snapshot) {
            calls.push(['restore', snapshot]);
            return false;
          },
          dispose() {
            calls.push('dispose');
          },
        },
      }),
      reload: (reason) => reloads.push(reason),
    }),
    /incompatible/,
  );

  assert.deepEqual(calls, [['restore', 'schema:1'], 'dispose']);
  assert.equal(reloads.length, 1);
  assert.match(reloads[0], /incompatible/);
});

test('requires snapshots to be strings', async () => {
  const captured = await captureWaluauGame(Promise.resolve({
    hotReplacement: {
      isRegistered: () => true,
      capture() {
        return { score: 7 };
      },
      dispose() {},
    },
  }));

  assert.equal(captured.ok, false);
  assert.match(captured.error.message, /capturing/);
});

test('bridges registered unit callbacks to a transient string snapshot', () => {
  const host = createWaluauHotHost();
  host.bindExports(() => ({
    __waluau_call_callback_unit(callback) {
      callback();
    },
  }));
  host.imports.__waluau_hmr_register(
    () => host.imports.__waluau_hmr_set_snapshot('schema:2'),
    () => host.imports.__waluau_hmr_set_restore_result(
      host.imports.__waluau_hmr_get_snapshot() === 'schema:2',
    ),
    () => {},
  );

  assert.equal(host.capture(), 'schema:2');
  assert.equal(host.restore('schema:2'), true);
  assert.equal(host.restore('schema:1'), false);
});
