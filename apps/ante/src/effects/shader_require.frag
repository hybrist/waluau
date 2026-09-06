precision highp float;

varying vec4 v_color;
varying vec2 v_uv;
uniform float u_time;

void main() {
    vec2 centered = v_uv * 2.0 - 1.0;
    float ring = 0.5 + 0.5 * cos(length(centered) * 18.0 - u_time * 3.0);
    vec3 ink = mix(vec3(0.12, 0.05, 0.30), vec3(0.25, 0.95, 0.82), ring);
    gl_FragColor = vec4(ink, 1.0) * v_color;
}
