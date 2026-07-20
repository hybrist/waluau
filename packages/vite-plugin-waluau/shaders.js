function assertSourceName(name) {
  if (typeof name !== 'string' || name.length === 0) {
    throw new TypeError('Waluau shader source names must be non-empty strings');
  }
}

function assertSourceText(name, source) {
  if (typeof source !== 'string') {
    throw new TypeError(`Waluau shader source "${name}" must be a string`);
  }
}

/**
 * Create the mutable host registry read by `waluau:engine/shader_sources`.
 *
 * Vite dependency updates mutate this object in place, so the imports already
 * bound to a running Wasm instance observe new revisions without replacing
 * that instance.
 *
 * @param {Record<string, string>} initialSources
 */
export function createWaluauShaderSourceHost(initialSources = {}) {
  if (initialSources == null || Array.isArray(initialSources) || typeof initialSources !== 'object') {
    throw new TypeError('Waluau shader sources must be a name-to-source object');
  }

  const sources = new Map();
  for (const [name, source] of Object.entries(initialSources)) {
    assertSourceName(name);
    assertSourceText(name, source);
    sources.set(name, { source, revision: 1 });
  }

  return {
    imports: {
      __waluau_shader_source_revision(name) {
        return sources.get(String(name))?.revision ?? -1;
      },
      __waluau_shader_source_text(name) {
        return sources.get(String(name))?.source ?? '';
      },
    },

    /**
     * Replace one configured source after Vite accepts its raw dependency.
     * Revisions advance only when the text changes.
     */
    update(name, source) {
      assertSourceName(name);
      assertSourceText(name, source);
      const current = sources.get(name);
      if (current == null) {
        throw new Error(`Unknown Waluau shader source "${name}"`);
      }
      if (current.source === source) return current.revision;
      current.source = source;
      current.revision += 1;
      return current.revision;
    },
  };
}
