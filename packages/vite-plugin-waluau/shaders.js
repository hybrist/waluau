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

const IMPORTED_SHADER_VERTEX_SOURCE = `
attribute vec2 a_position;
attribute vec4 a_color;
attribute vec2 a_uv;
attribute float a_textured;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
    v_color = a_color;
    v_uv = a_uv;
    v_textured = a_textured;
}
`;

function shaderError(code, message) {
  return { code, message };
}

function compileStage(gl, kind, source, stage) {
  const shader = gl.createShader(kind);
  if (shader == null) return { shader: null, error: shaderError(stage, `could not create ${stage} shader`) };
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const message = gl.getShaderInfoLog(shader) || `unknown ${stage} shader compile failure`;
    gl.deleteShader(shader);
    return { shader: null, error: shaderError(`${stage}_compile`, message) };
  }
  return { shader, error: null };
}

/**
 * One Vite-owned fragment shader. Its WebGLProgram belongs to the single
 * engine canvas rather than to any Wasm generation using that canvas.
 */
export function createWaluauImportedShader(source, sourcePath = '<imported shader>') {
  assertSourceText(sourcePath, source);
  let gl = null;
  let program = null;
  let sourceRevision = 1;
  let compiledSourceRevision = 0;
  let failedSourceRevision = 0;
  let programRevision = 0;
  let failure = shaderError('', '');

  const compile = (context) => {
    if (gl != null && gl !== context) {
      failure = shaderError(
        'context',
        `${sourcePath} is already associated with a different WebGL2 context; reload the page`,
      );
      globalThis.location?.reload?.();
      return false;
    }
    gl = context;
    if (program != null && compiledSourceRevision === sourceRevision) return true;
    if (failedSourceRevision === sourceRevision) return false;

    const vertex = compileStage(gl, gl.VERTEX_SHADER, IMPORTED_SHADER_VERTEX_SOURCE, 'vertex');
    if (vertex.shader == null) {
      failedSourceRevision = sourceRevision;
      failure = vertex.error;
      return false;
    }
    const fragment = compileStage(gl, gl.FRAGMENT_SHADER, source, 'pixel');
    if (fragment.shader == null) {
      gl.deleteShader(vertex.shader);
      failedSourceRevision = sourceRevision;
      failure = fragment.error;
      return false;
    }

    const candidate = gl.createProgram();
    if (candidate == null) {
      gl.deleteShader(vertex.shader);
      gl.deleteShader(fragment.shader);
      failedSourceRevision = sourceRevision;
      failure = shaderError('link', 'could not create shader program');
      return false;
    }
    gl.attachShader(candidate, vertex.shader);
    gl.attachShader(candidate, fragment.shader);
    gl.linkProgram(candidate);
    gl.deleteShader(vertex.shader);
    gl.deleteShader(fragment.shader);
    if (!gl.getProgramParameter(candidate, gl.LINK_STATUS)) {
      const message = gl.getProgramInfoLog(candidate) || 'unknown WebGL shader link failure';
      gl.deleteProgram(candidate);
      failedSourceRevision = sourceRevision;
      failure = shaderError('link', message);
      return false;
    }

    const previous = program;
    program = candidate;
    compiledSourceRevision = sourceRevision;
    failedSourceRevision = 0;
    programRevision += 1;
    failure = shaderError('', '');
    if (previous != null) gl.deleteProgram(previous);
    return true;
  };

  return {
    compile,
    update(nextSource) {
      assertSourceText(sourcePath, nextSource);
      if (source === nextSource) return true;
      source = nextSource;
      sourceRevision += 1;
      failedSourceRevision = 0;
      return gl == null ? true : compile(gl);
    },
    program: () => program,
    revision: () => programRevision,
    error: () => failure,
    release(context) {
      if (gl == null || gl !== context) return;
      if (program != null) gl.deleteProgram(program);
      gl = null;
      program = null;
      compiledSourceRevision = 0;
      failedSourceRevision = 0;
      failure = shaderError('', '');
    },
  };
}

/** Build the host imports for compiler-generated shader require functions. */
export function createWaluauImportedShaderHost(shadersByImportName = {}) {
  const shaders = Object.values(shadersByImportName);
  const checked = (shader) => {
    if (!shaders.includes(shader)) throw new TypeError('Unknown imported Waluau shader handle');
    return shader;
  };
  const imports = {
    game_imported_shader_compile: (shader, gl) => checked(shader).compile(gl) ? 1 : 0,
    game_imported_shader_program: (shader) => checked(shader).program(),
    game_imported_shader_revision: (shader) => checked(shader).revision(),
    game_imported_shader_error_code: (shader) => checked(shader).error().code,
    game_imported_shader_error_message: (shader) => checked(shader).error().message,
    game_imported_shaders_release: (gl) => {
      for (const shader of shaders) shader.release(gl);
    },
  };
  for (const [importName, shader] of Object.entries(shadersByImportName)) {
    imports[importName] = () => shader;
  }
  return {
    imports,
  };
}
