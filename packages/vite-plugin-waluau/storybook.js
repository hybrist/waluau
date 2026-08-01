// Browser half of engine/storybook.walu. A story file's Wasm module publishes
// one mount closure per story and one teardown closure; both cross as ordinary
// () -> unit callbacks through the compiler's trampoline, the same bridge the
// test and hot-replacement hosts use.
//
// The Storybook renderer (@waluau/storybook) drives this: it asks the book to
// mount a story into the element Storybook gave it, and tears the canvas down
// when the story is replaced by one from another file.

const CALLBACK_UNIT_TRAMPOLINE_EXPORT = '__waluau_call_callback_unit';
const LUA_ERROR_TAG_EXPORT = '__waluau_error_tag';

// The element the engine mounts its canvas inside. The renderer creates it
// under Storybook's canvas element; engine/storybook.walu reads the id back
// through __waluau_storybook_mount at mount time.
export const WALUAU_STORY_VIEWPORT_ID = 'walua-story-viewport';

/** Rethrows a Waluau `error`/`assert` as a plain JS error carrying its message. */
function normalizeWaluError(error, getWasmExports) {
  const tag = getWasmExports()?.[LUA_ERROR_TAG_EXPORT];
  if (
    typeof WebAssembly.Exception === 'function' &&
    error instanceof WebAssembly.Exception &&
    tag instanceof WebAssembly.Tag &&
    error.is(tag)
  ) {
    return new Error(String(error.getArg(tag, 0)));
  }
  return error;
}

/**
 * A story file's book: the compiled module plus the stories it published.
 *
 * @param {{
 *   run: (options?: object) => Promise<{ exports: object }>,
 *   createImports: (context: object, hostImports: object) => object,
 *   wasmUrl?: URL | null,
 * }} options
 */
export function createWaluauBook({ run, createImports, wasmUrl }) {
  const shows = new Map();
  let teardown = null;
  let getWasmExports = () => null;
  let loading = null;
  let mountedElement = null;
  let currentArgs = {};

  const invoke = (callback) => {
    const trampoline = getWasmExports()?.[CALLBACK_UNIT_TRAMPOLINE_EXPORT];
    if (typeof trampoline !== 'function') {
      throw new Error(`Missing ${CALLBACK_UNIT_TRAMPOLINE_EXPORT} export for a Waluau story`);
    }
    try {
      trampoline(callback);
    } catch (error) {
      throw normalizeWaluError(error, getWasmExports);
    }
  };

  const hostImports = {
    __waluau_storybook_story: (name, show) => {
      shows.set(String(name), show);
    },
    __waluau_storybook_teardown: (hide) => {
      teardown = hide;
    },
    __waluau_storybook_mount: () => WALUAU_STORY_VIEWPORT_ID,
    __waluau_storybook_arg_i32: (name) => {
      const key = String(name);
      const value = currentArgs[key];
      if (!Number.isInteger(value) || value < -2147483648 || value > 2147483647) {
        throw new Error(`The Waluau Storybook arg "${key}" is not an i32`);
      }
      return value;
    },
  };

  // The module's top level publishes every story, so loading it once is what
  // fills `shows`. Storybook renders one story at a time out of the same
  // module, and a failed load must not be cached as a resolved book.
  const load = () => {
    loading ??= run({
      wasmUrl,
      createImports: (context) => {
        getWasmExports = context.getWasmExports;
        return createImports(context, hostImports);
      },
    }).catch((error) => {
      loading = null;
      throw normalizeWaluError(error, getWasmExports);
    });
    return loading;
  };

  // Storybook hands the renderer a fresh canvas element per render; the engine
  // needs a stable, empty element of its own inside it.
  const viewport = (canvasElement) => {
    const existing = canvasElement.querySelector(`#${WALUAU_STORY_VIEWPORT_ID}`);
    if (existing != null) return existing;
    const element = document.createElement('div');
    element.id = WALUAU_STORY_VIEWPORT_ID;
    element.setAttribute(
      'style',
      'display:flex;width:100%;height:100%;min-height:100%;align-items:center;justify-content:center;',
    );
    canvasElement.appendChild(element);
    return element;
  };

  // Storybook tears the current story down before rendering the next one, even
  // when the next one comes out of this same file. Releasing the canvas there
  // and then would throw away the WebGL context — and every texture, font and
  // shader the story file loaded into it — between two stories that are about
  // to share it. So a teardown waits one task: if another story claims the
  // book first, nothing was lost, and if none does, the session really stops.
  let pendingTeardown = null;
  const release = () => {
    pendingTeardown = null;
    if (mountedElement == null) return;
    const element = mountedElement;
    mountedElement = null;
    if (teardown != null) invoke(teardown);
    element.querySelector(`#${WALUAU_STORY_VIEWPORT_ID}`)?.remove();
  };

  return {
    /** The story names this file published; empty until the module has loaded. */
    names() {
      return Array.from(shows.keys());
    },
    async mount(name, canvasElement, args = {}) {
      if (pendingTeardown != null) {
        clearTimeout(pendingTeardown);
        pendingTeardown = null;
      }
      currentArgs = args ?? {};
      await load();
      const show = shows.get(name);
      if (show === undefined) {
        const published = Array.from(shows.keys()).map((key) => `"${key}"`).join(', ');
        throw new Error(
          `The story "${name}" is not published by this Waluau story file (published: ${published || 'none'})`,
        );
      }
      // Storybook replaces the canvas element when the preview remounts; the
      // old session's canvas went with it, so it has to be released first.
      if (mountedElement != null && mountedElement !== canvasElement) release();
      mountedElement = canvasElement;
      viewport(canvasElement);
      invoke(show);
    },
    unmount() {
      if (mountedElement == null || pendingTeardown != null) return;
      pendingTeardown = setTimeout(release, 0);
    },
  };
}
