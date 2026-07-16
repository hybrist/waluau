import { describe, expect, it } from 'vitest';
import { createGameServicesHost } from '../../playground/src/utils/wasm.js';

class FakeFontFace {
  constructor(family) {
    this.family = family;
  }

  async load() {
    return this;
  }
}

class FakeAudioContext {
  constructor() {
    this.destination = {};
  }

  async decodeAudioData(bytes) {
    return { byteLength: bytes.byteLength };
  }

  createBufferSource() {
    return {
      connect() {},
      disconnect() {},
      start() {},
      stop() { this.onended?.(); },
    };
  }

  createGain() {
    return { gain: { value: 1 }, connect() {} };
  }

  async resume() {}
}

class FakeAudio {
  constructor(url) {
    this.url = url;
    this.readyState = 0;
    this.paused = true;
    this.ended = false;
    this.currentTime = 0;
    this.listeners = new Map();
  }

  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }

  load() {
    queueMicrotask(() => {
      this.readyState = 4;
      this.listeners.get('canplaythrough')?.();
    });
  }

  async play() {
    this.paused = false;
  }

  pause() {
    this.paused = true;
  }

  removeAttribute(name) {
    if (name === 'src') this.url = '';
  }
}

function mapStorage() {
  const values = new Map();
  return {
    getItem: (key) => values.get(String(key)) ?? null,
    setItem: (key, value) => values.set(String(key), String(value)),
    removeItem: (key) => values.delete(String(key)),
  };
}

function serviceHarness() {
  const bitmap = { closed: false, close() { this.closed = true; } };
  const fontSet = new Set();
  const bodies = new Map([
    ['/assets/message.txt', 'packaged text'],
    ['/assets/data.bin', new Uint8Array([1, 2, 3, 4])],
    ['/assets/card-back.svg', '<svg xmlns="http://www.w3.org/2000/svg"/>'],
    ['/assets/game.woff2', new Uint8Array([5, 6, 7])],
    ['/assets/click.wav', new Uint8Array([8, 9, 10])],
  ]);
  const fetch = async (url) => {
    const path = new URL(url).pathname;
    if (!bodies.has(path)) return new Response('', { status: 404 });
    return new Response(bodies.get(path), { status: 200 });
  };

  return {
    bitmap,
    fontSet,
    host: createGameServicesHost({
      assetBaseUrl: 'https://game.test/',
      saveNamespace: 'sample-game',
      storage: mapStorage(),
      fetch,
      createImageBitmap: async () => bitmap,
      FontFace: FakeFontFace,
      fontSet,
      AudioContext: FakeAudioContext,
      Audio: FakeAudio,
    }),
  };
}

describe('browser game resource services', () => {
  it('loads packaged text, bytes, images and fonts with explicit lifetime', async () => {
    const { host, bitmap, fontSet } = serviceHarness();
    const text = await host.game_resource_load_text('/assets/message.txt');
    const bytes = await host.game_resource_load_bytes('/assets/data.bin');
    const image = await host.game_resource_load_image('/assets/card-back.svg');
    const font = await host.game_resource_load_font('/assets/game.woff2', 'Sample Game');

    expect(host.game_resource_text(text)).toBe('packaged text');
    expect(Array.from(host.game_resource_bytes(bytes))).toEqual([1, 2, 3, 4]);
    expect(host.game_resource_font_family(font)).toBe('Sample Game');
    expect(fontSet.size).toBe(1);

    host.game_resource_release(image);
    host.game_resource_release(image);
    host.game_resource_release(font);
    expect(bitmap.closed).toBe(true);
    expect(fontSet.size).toBe(0);
  });

  it('resolves failures to structured handles instead of rejecting', async () => {
    const { host } = serviceHarness();
    const missing = await host.game_resource_load_text('/assets/missing.txt');
    const invalid = await host.game_resource_load_text('../secret.txt');

    expect(host.game_resource_ok(missing)).toBe(false);
    expect(host.game_resource_error_code(missing)).toBe('not_found');
    expect(host.game_resource_error_code(invalid)).toBe('invalid_path');
    expect(host.game_resource_error_message(missing)).toContain('missing.txt');
  });

  it('decodes effects, readies streams and controls playback safely', async () => {
    const { host } = serviceHarness();
    const sound = await host.game_audio_load_sound('/assets/click.wav');
    const stream = await host.game_audio_load_stream('/assets/music.ogg');

    expect(host.game_resource_ok(sound)).toBe(true);
    expect(host.game_resource_ok(stream)).toBe(true);
    expect(host.game_audio_play(sound, false, 0.5)).toBe(true);
    expect(host.game_audio_is_playing(sound)).toBe(true);
    host.game_audio_stop(sound);
    expect(host.game_audio_is_playing(sound)).toBe(false);
    expect(host.game_audio_play(stream, true, 0.25)).toBe(true);
    host.game_audio_pause(stream);
    expect(host.game_audio_is_playing(stream)).toBe(false);
  });

  it('keeps namespaced save text and bytes separate from packaged assets', async () => {
    const { host } = serviceHarness();
    expect(host.game_resource_ok(await host.game_save_write_text('progress', 'breach=3'))).toBe(true);
    const text = await host.game_save_read_text('progress');
    expect(host.game_resource_text(text)).toBe('breach=3');

    expect(host.game_resource_ok(
      await host.game_save_write_bytes('snapshot', new Uint8Array([7, 8, 9]))
    )).toBe(true);
    const bytes = await host.game_save_read_bytes('snapshot');
    expect(Array.from(host.game_resource_bytes(bytes))).toEqual([7, 8, 9]);

    const wrongType = await host.game_save_read_text('snapshot');
    expect(host.game_resource_error_code(wrongType)).toBe('wrong_type');
    const invalid = await host.game_save_read_text('../outside');
    expect(host.game_resource_error_code(invalid)).toBe('invalid_key');
    expect(host.game_resource_ok(await host.game_save_delete('progress'))).toBe(true);
    const missing = await host.game_save_read_text('progress');
    expect(host.game_resource_error_code(missing)).toBe('not_found');
  });
});
