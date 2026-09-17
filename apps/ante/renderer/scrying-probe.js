// Observe the production lens and its sampled texture, without exposing game state.
export async function installScryingProbe(page) {
  await page.addInitScript(() => {
    const gl = WebGL2RenderingContext.prototype;
    const shaders = new WeakSet();
    const programs = new WeakSet();
    const uniforms = new WeakMap();
    const textures = new WeakMap();
    const original = Object.fromEntries([
      'shaderSource', 'attachShader', 'getUniformLocation', 'uniform1f',
      'texImage2D', 'drawArrays',
    ].map((name) => [name, gl[name]]));
    window.scryingProbe = { clocks: [], captures: [] };
    gl.shaderSource = function (shader, source) {
      if (source.includes('u_static_circle')) shaders.add(shader);
      return original.shaderSource.call(this, shader, source);
    };
    gl.attachShader = function (program, shader) {
      if (shaders.has(shader)) programs.add(program);
      return original.attachShader.call(this, program, shader);
    };
    gl.getUniformLocation = function (program, name) {
      const location = original.getUniformLocation.call(this, program, name);
      if (location && programs.has(program)) uniforms.set(location, name);
      return location;
    };
    gl.uniform1f = function (location, value) {
      if (uniforms.get(location) === 'u_clock') {
        window.scryingProbe.clocks.push(value);
      }
      return original.uniform1f.call(this, location, value);
    };
    gl.texImage2D = function (...args) {
      if (args.length === 9) {
        textures.set(this.getParameter(this.TEXTURE_BINDING_2D), [args[3], args[4]]);
      }
      return original.texImage2D.apply(this, args);
    };
    gl.drawArrays = function (...args) {
      if (programs.has(this.getParameter(this.CURRENT_PROGRAM))) {
        const size = textures.get(this.getParameter(this.TEXTURE_BINDING_2D));
        if (size) window.scryingProbe.captures.push(size);
      }
      return original.drawArrays.apply(this, args);
    };
  });
}
