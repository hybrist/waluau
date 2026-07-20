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
