const CALLBACK_UNIT_TRAMPOLINE_EXPORT = '__waluau_call_callback_unit';

export class HotReplacementFallback extends Error {
  constructor(message, options) {
    super(message, options);
    this.name = 'HotReplacementFallback';
  }
}

function fallback(message, cause) {
  return {
    ok: false,
    error: new HotReplacementFallback(message, cause == null ? undefined : { cause }),
  };
}

/**
 * Host half of engine/hot.walu's callback registration. All three guest
 * callbacks use the compiler's existing () -> unit trampoline; snapshot data
 * and restore status cross through small string/bool imports.
 */
export function createWaluauHotHost() {
  let getExports = () => null;
  let callbacks = null;
  let snapshotValue;
  let restoreAccepted = false;
  let restoreCompleted = false;

  const invoke = (callback) => {
    const trampoline = getExports()?.[CALLBACK_UNIT_TRAMPOLINE_EXPORT];
    if (typeof trampoline !== 'function') {
      throw new Error(`Missing ${CALLBACK_UNIT_TRAMPOLINE_EXPORT} export for hot replacement`);
    }
    trampoline(callback);
  };

  return {
    bindExports(getter) {
      getExports = typeof getter === 'function' ? getter : () => null;
    },
    isRegistered() {
      return callbacks != null;
    },
    imports: {
      __waluau_hmr_register(capture, restore, dispose) {
        callbacks = { capture, restore, dispose };
      },
      __waluau_hmr_set_snapshot(value) {
        snapshotValue = value;
      },
      __waluau_hmr_get_snapshot() {
        return snapshotValue;
      },
      __waluau_hmr_set_restore_result(accepted) {
        restoreAccepted = Boolean(accepted);
        restoreCompleted = true;
      },
    },
    capture() {
      if (callbacks == null) {
        throw new Error('The game did not register development hot-replacement hooks');
      }
      snapshotValue = undefined;
      invoke(callbacks.capture);
      if (typeof snapshotValue !== 'string') {
        throw new TypeError('The game hot-replacement snapshot must be a string');
      }
      return snapshotValue;
    },
    restore(snapshot) {
      if (callbacks == null) {
        throw new Error('The replacement game did not register hot-replacement hooks');
      }
      snapshotValue = snapshot;
      restoreAccepted = false;
      restoreCompleted = false;
      invoke(callbacks.restore);
      if (!restoreCompleted) {
        throw new Error('The game restore hook did not report a compatibility result');
      }
      return restoreAccepted;
    },
    dispose() {
      if (callbacks == null) return;
      const dispose = callbacks.dispose;
      callbacks = null;
      invoke(dispose);
    },
  };
}

/**
 * Capture and stop a running game while Vite disposes its old JavaScript
 * module. The returned promise is stored in import.meta.hot.data for the
 * replacement module.
 */
export async function captureWaluauGame(gamePromise) {
  let loaded;
  try {
    loaded = await gamePromise;
  } catch (error) {
    return fallback('The previous Waluau game did not finish starting', error);
  }

  const hot = loaded?.hotReplacement;
  if (hot == null || !hot.isRegistered()) {
    return fallback('The previous game did not register development hot-replacement hooks');
  }

  let value;
  let captureError;
  try {
    value = hot.capture();
    if (typeof value !== 'string') {
      throw new TypeError('The game hot-replacement snapshot must be a string');
    }
  } catch (error) {
    captureError = error;
  }

  try {
    hot.dispose();
  } catch (error) {
    return fallback('The previous game failed while disposing its browser session', error);
  }

  if (captureError != null) {
    return fallback('The previous game failed while capturing its state', captureError);
  }
  return { ok: true, snapshot: value };
}

async function disposeReplacement(loaded) {
  const hot = loaded?.hotReplacement;
  if (hot == null) return;
  try {
    hot.dispose();
  } catch (error) {
    console.warn('Waluau game disposal failed during reload fallback', error);
  }
}

/**
 * Start an initial game or restore a freshly compiled game from the snapshot
 * captured by captureWaluauGame. Any unsupported or incompatible transition
 * invalidates the Vite module so the browser performs a full reload.
 */
export async function replaceWaluauGame({ previous, start, reload }) {
  if (previous == null) return start();

  const captured = await previous;
  if (!captured?.ok) {
    reload(captured?.error?.message ?? 'Waluau hot replacement could not capture game state');
    throw captured?.error ?? new HotReplacementFallback('Waluau hot replacement failed');
  }

  let loaded;
  try {
    loaded = await start();
  } catch (error) {
    reload('The replacement Waluau game did not finish starting');
    throw new HotReplacementFallback(
      'The replacement Waluau game did not finish starting',
      { cause: error },
    );
  }

  const hot = loaded?.hotReplacement;
  if (hot == null || !hot.isRegistered()) {
    await disposeReplacement(loaded);
    const error = new HotReplacementFallback(
      'The replacement game did not register development hot-replacement hooks',
    );
    reload(error.message);
    throw error;
  }

  try {
    if (hot.restore(captured.snapshot) !== true) {
      throw new HotReplacementFallback(
        'The replacement game rejected the previous snapshot as incompatible',
      );
    }
  } catch (error) {
    await disposeReplacement(loaded);
    const fallbackError = error instanceof HotReplacementFallback
      ? error
      : new HotReplacementFallback(
        'The replacement game failed while restoring its state',
        { cause: error },
      );
    reload(fallbackError.message);
    throw fallbackError;
  }

  return loaded;
}
