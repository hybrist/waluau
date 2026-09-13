import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../src/city_scene_shaders.walu', import.meta.url), 'utf8');
const vertex = source.match(/const VERTEX: string = \[\[([\s\S]*?)\]\]/)[1].trim();
const pixel = source.match(/const PIXEL: string = \[\[([\s\S]*?)\]\]/)[1].trim();

test('keeps thin city shadow layers visible throughout camera movement', async ({ page }) => {
  await page.goto('/');
  const result = await page.evaluate(({ vertexSource, pixelSource }) => {
    const canvas = document.createElement('canvas');
    canvas.width = 256;
    canvas.height = 256;
    const gl = canvas.getContext('webgl2');
    function compile(type, source) {
      const shader = gl.createShader(type);
      gl.shaderSource(shader, source);
      gl.compileShader(shader);
      if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(shader));
      return shader;
    }
    const program = gl.createProgram();
    gl.attachShader(program, compile(gl.VERTEX_SHADER, vertexSource));
    gl.attachShader(program, compile(gl.FRAGMENT_SHADER, pixelSource));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program));
    gl.useProgram(program);
    const vertices = [];
    function quad(radius, z, color, cx = 1280, cy = 860) {
      for (const [x, y] of [[-1, -1], [1, -1], [1, 1], [-1, -1], [1, 1], [-1, 1]]) {
        vertices.push(cx + x * radius, cy + y * radius, z, ...color, 8, 0, 0);
      }
    }
    for (let y = -2000; y < 4000; y += 250) {
      for (let x = -2000; x < 4500; x += 250) quad(125, -0.2, [1, 0, 0], x + 125, y + 125);
    }
    quad(100, -0.1, [0, 1, 0]);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(vertices), gl.STATIC_DRAW);
    for (const [name, size, offset] of [['a_position', 3, 0], ['a_color', 4, 12]]) {
      const location = gl.getAttribLocation(program, name);
      gl.enableVertexAttribArray(location);
      gl.vertexAttribPointer(location, size, gl.FLOAT, false, 36, offset);
    }
    gl.uniform1f(gl.getUniformLocation(program, 'u_alpha'), 1);
    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LESS);
    gl.uniform4f(gl.getUniformLocation(program, 'u_view'), 256, 256, 128, 128);
    const failures = [];
    const pixel = new Uint8Array(4);
    // Tiled paving under a small shadow, with the production city shaders.
    // Sweep subpixel camera positions at menu, distant, and dive distances.
    for (const distance of [30, 300, 900, 1700, 2500]) {
      for (let frame = 0; frame < 120; frame++) {
        const dx = Math.sin(frame * 0.07) * distance * 0.04;
        const dy = Math.cos(frame * 0.09) * distance * 0.04;
        gl.uniform3f(gl.getUniformLocation(program, 'u_camera'), 1280 + dx, 860 + dy, distance);
        gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
        gl.drawArrays(gl.TRIANGLES, 0, vertices.length / 9);
        const depth = distance + dy * 0.55 + 0.1 * 0.835165;
        const x = Math.floor(128 - dx * 900 / depth);
        const y = Math.floor(128 + (dy * 0.835165 - 0.1 * 0.55) * 900 / depth);
        gl.readPixels(x, y, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
        if (pixel[1] <= pixel[0] * 1.5) failures.push({ distance, frame, pixel: [...pixel] });
      }
    }
    return { failures, depthBits: gl.getParameter(gl.DEPTH_BITS), error: gl.getError() };
  }, { vertexSource: vertex, pixelSource: pixel });
  expect(result.error).toBe(0);
  expect(result.failures.length, `Depth buffer: ${result.depthBits} bits; first failures: ${JSON.stringify(result.failures.slice(0, 3))}`).toBe(0);
});
