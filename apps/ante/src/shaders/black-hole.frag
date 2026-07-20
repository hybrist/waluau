precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_aspect;
uniform float u_selected;
uniform float u_colored;
void main() {
    vec2 p = (v_uv - 0.5) * 2.0;
    p.x *= u_aspect;
    float radius = length(p);
    float angle = atan(p.y, p.x);
    float pulse = 0.92 + 0.08 * sin(u_time * 3.1);
    float spin = angle * 3.0 - radius * 11.0
        + 2.5 / (radius + 0.16) + u_time * 2.0;
    float arm_phase = spin + sin(angle * 2.0 + u_time) * 0.7;
    float filaments = 1.0 - smoothstep(0.045, 0.145, abs(sin(arm_phase)));
    float fine_filaments = 1.0 - smoothstep(
        0.030, 0.105,
        abs(sin(arm_phase * 1.7 + radius * 5.0 - u_time * 0.45))
    );
    float disk_mask = smoothstep(0.27, 0.34, radius)
        * (1.0 - smoothstep(0.82, 1.02, radius));
    filaments *= disk_mask;
    fine_filaments *= disk_mask * (1.0 - smoothstep(0.62, 0.92, radius));

    float lens_ring = 1.0 - smoothstep(0.012, 0.045, abs(radius - 0.335));
    float outer_ring = 1.0 - smoothstep(0.018, 0.060, abs(radius - 0.68));
    float wisp_phase = angle * 7.0 - radius * 18.0 + u_time * 1.7;
    float sharp_wisps = (1.0 - smoothstep(0.040, 0.125, abs(sin(wisp_phase))))
        * smoothstep(0.37, 0.46, radius)
        * (1.0 - smoothstep(0.68, 0.80, radius));
    float intensity = 0.88 * pulse + u_selected * 0.30;

    vec3 color = v_color.rgb * 0.07 + vec3(0.008, 0.0, 0.020);
    color += vec3(0.11, 0.008, 0.30) * disk_mask * 0.30;
    color += vec3(0.54, 0.08, 0.96) * filaments * intensity;
    color += vec3(0.34, 0.035, 0.72) * fine_filaments * intensity * 0.72;
    color += vec3(0.48, 0.08, 0.88) * sharp_wisps * 0.62;
    color += vec3(0.78, 0.34, 0.94) * lens_ring * (0.82 + u_selected * 0.24);
    color += vec3(0.34, 0.06, 0.72) * outer_ring * 0.42;

    // Narrow lensing lines frame a hard, truly black event horizon.
    float outside_void = smoothstep(0.22, 0.275, radius);
    color *= outside_void;
    color += vec3(0.008, 0.0, 0.018) * smoothstep(0.25, 0.18, radius);
    float grayscale = dot(color, vec3(0.299, 0.587, 0.114));
    color = mix(vec3(grayscale), color, u_colored);

    vec4 output_color = vec4(color, v_color.a);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
