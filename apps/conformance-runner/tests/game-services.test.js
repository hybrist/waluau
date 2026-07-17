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
  const bitmap = { width: 2, height: 2, closed: false, close() { this.closed = true; } };
  const fontAtlas = { atlas: true };
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
    fontAtlas,
    fontSet,
    host: createGameServicesHost({
      assetBaseUrl: 'https://game.test/',
      saveNamespace: 'sample-game',
      storage: mapStorage(),
      fetch,
      createImageBitmap: async () => bitmap,
      FontFace: FakeFontFace,
      fontSet,
      createFontAtlas: () => ({
        source: fontAtlas,
        width: 640,
        height: 264,
        firstCode: 32,
        glyphCount: 95,
        columns: 16,
        cellWidth: 40,
        cellHeight: 44,
        baseSize: 32,
        advance: 32,
      }),
      AudioContext: FakeAudioContext,
      Audio: FakeAudio,
    }),
  };
}

describe('browser game resource services', () => {
  it('falls back to an HTML image when createImageBitmap cannot decode SVG', async () => {
    let decoded = false;
    let revoked = '';
    class FakeImage {
      constructor() {
        this.width = 104;
        this.height = 128;
        this.src = '';
      }

      async decode() {
        decoded = true;
      }
    }
    const host = createGameServicesHost({
      assetBaseUrl: 'https://game.test/',
      fetch: async () => new Response('<svg xmlns="http://www.w3.org/2000/svg"/>', {
        status: 200,
        headers: { 'Content-Type': 'image/svg+xml' },
      }),
      createImageBitmap: async () => {
        throw new DOMException('The source image could not be decoded.', 'InvalidStateError');
      },
      Image: FakeImage,
      createObjectURL: () => 'blob:card-back',
      revokeObjectURL: (url) => { revoked = url; },
    });

    const image = await host.game_resource_load_image('/assets/card-back.svg');
    expect(host.game_resource_ok(image)).toBe(true);
    expect(decoded).toBe(true);
    expect(revoked).toBe('blob:card-back');
  });

  it('copies decoded images into opaque GPU handles with structured lifetime errors', async () => {
    const { host, bitmap, fontAtlas, fontSet } = serviceHarness();
    const calls = [];
    const gl = {
      TEXTURE_2D: 1, TEXTURE_MIN_FILTER: 2, TEXTURE_MAG_FILTER: 3,
      TEXTURE_WRAP_S: 4, TEXTURE_WRAP_T: 5, NEAREST: 6, CLAMP_TO_EDGE: 7,
      UNPACK_PREMULTIPLY_ALPHA_WEBGL: 8, RGBA: 9, UNSIGNED_BYTE: 10,
      TEXTURE0: 11, FRAMEBUFFER: 12, COLOR_ATTACHMENT0: 13, FRAMEBUFFER_COMPLETE: 14,
      drawingBufferWidth: 160, drawingBufferHeight: 100,
      createTexture: () => ({ texture: true }), createFramebuffer: () => ({ framebuffer: true }),
      bindTexture: (...args) => calls.push(['bindTexture', ...args]),
      texParameteri: () => {}, pixelStorei: () => {},
      texImage2D: (...args) => calls.push(['texImage2D', ...args]),
      bindFramebuffer: () => {}, framebufferTexture2D: () => {},
      checkFramebufferStatus: () => 14, activeTexture: () => {}, viewport: () => {},
      deleteTexture: (...args) => calls.push(['deleteTexture', ...args]),
      deleteFramebuffer: () => {},
    };
    const image = await host.game_resource_load_image('/assets/card-back.svg');
    const texture = host.game_gpu_texture_from_resource(gl, image);
    expect(host.game_gpu_resource_ok(texture)).toBe(true);
    expect(host.game_gpu_resource_width(texture)).toBe(2);
    expect(calls.find(([name]) => name === 'texImage2D').at(-1)).toBe(bitmap);
    host.game_resource_release(image);
    expect(bitmap.closed).toBe(true);
    expect(host.game_gpu_texture_bind(gl, texture)).toBe(true);
    host.game_gpu_resource_release(texture);
    host.game_gpu_resource_release(texture);
    expect(host.game_gpu_resource_error_code(texture)).toBe('released');
    expect(host.game_gpu_texture_bind(gl, texture)).toBe(false);

    const invalid = host.game_gpu_texture_from_resource(gl, image);
    expect(host.game_gpu_resource_ok(invalid)).toBe(false);
    expect(host.game_gpu_resource_error_code(invalid)).toBe('invalid_resource');
    const badTarget = host.game_gpu_render_target_create(gl, 0, 16);
    expect(host.game_gpu_resource_error_code(badTarget)).toBe('invalid_size');

    const loadedFont = await host.game_resource_load_font('/assets/game.woff2', 'Sample Game');
    const gpuFont = host.game_gpu_font_from_resource(gl, loadedFont);
    expect(host.game_gpu_resource_ok(gpuFont)).toBe(true);
    expect(host.game_gpu_font_first_code(gpuFont)).toBe(32);
    expect(host.game_gpu_font_glyph_count(gpuFont)).toBe(95);
    expect(host.game_gpu_font_advance(gpuFont)).toBe(32);
    expect(calls.find((call) => call.at(-1) === fontAtlas)).toBeDefined();
    host.game_resource_release(loadedFont);
    expect(fontSet.size).toBe(0);
    expect(host.game_gpu_texture_bind(gl, gpuFont)).toBe(true);
    host.game_gpu_resource_release(gpuFont);
    host.game_gpu_resource_release(gpuFont);
    expect(host.game_gpu_resource_error_code(gpuFont)).toBe('released');
  });

  it('resolves logical packaged paths through typed fingerprint manifests', async () => {
    const requested = [];
    const host = createGameServicesHost({
      assetBaseUrl: 'https://game.test/dist/',
      assetManifest: {
        'assets/message.txt': { url: './assets/message.a1b2.txt', type: 'text' },
        'assets/data.bin': { url: './assets/data.c3d4.bin', type: 'bytes' },
      },
      fetch: async (url) => {
        requested.push(url);
        return new Response(url.endsWith('.txt') ? 'manifest text' : new Uint8Array([4, 2]), {
          status: 200,
        });
      },
      storage: mapStorage(),
    });

    const text = await host.game_resource_load_text('assets/message.txt');
    const bytes = await host.game_resource_load_bytes('assets/data.bin');
    expect(host.game_resource_text(text)).toBe('manifest text');
    expect(Array.from(host.game_resource_bytes(bytes))).toEqual([4, 2]);
    expect(requested).toEqual([
      'https://game.test/dist/assets/message.a1b2.txt',
      'https://game.test/dist/assets/data.c3d4.bin',
    ]);

    const undeclared = await host.game_resource_load_text('assets/missing.txt');
    const wrongType = await host.game_resource_load_image('assets/message.txt');
    expect(host.game_resource_error_code(undeclared)).toBe('undeclared_asset');
    expect(host.game_resource_error_code(wrongType)).toBe('wrong_asset_type');

    expect(host.game_resource_ok(
      await host.game_save_write_text('progress', 'separate-from-assets')
    )).toBe(true);
  });

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
