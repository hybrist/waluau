precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_aspect;

float hash21(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

float fbm(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    mat2 rot = mat2(0.80, 0.60, -0.60, 0.80);
    for (int i = 0; i < 4; i++) {
        v += a * noise(p);
        p = rot * p * 2.02 + vec2(1.2, 3.4);
        a *= 0.5;
    }
    return v;
}

void main() {
    vec2 p = (v_uv - 0.5) * 2.0;
    p.x *= u_aspect;
    float radius = length(p);

    // -------------------------------------------------------------------------
    // Astral Sea: Domain-warped cosmic nebula clouds
    // -------------------------------------------------------------------------
    // Slow, hypnotic harmonic currents drift through the astral ether.
    vec2 warp_a = vec2(
        fbm(p * 1.1 + vec2(u_time * 0.022, -u_time * 0.016)),
        fbm(p * 1.1 + vec2(-u_time * 0.019, u_time * 0.024) + vec2(4.3, 1.7))
    );

    vec2 warp_b = vec2(
        fbm(p * 1.5 + 2.2 * warp_a + vec2(1.9, u_time * 0.014)),
        fbm(p * 1.5 + 2.2 * warp_a + vec2(6.8, -u_time * 0.018))
    );

    float nebula_magenta = fbm(p * 1.25 + 1.8 * warp_b);
    float nebula_cyan = fbm(p * 1.75 - 1.4 * warp_a + vec2(3.7, u_time * 0.012));
    float nebula_violet = fbm(p * 0.90 + 2.4 * warp_b + vec2(-2.1, -u_time * 0.016));

    // -------------------------------------------------------------------------
    // Undulating Astral Waves and Celestial Caustics
    // -------------------------------------------------------------------------
    float sea_wave1 = pow(
        1.0 - abs(sin(p.x * 1.8 + p.y * 1.2 + sin(p.x * 2.5 + u_time * 0.12) * 0.7 - u_time * 0.10)),
        5.0
    );
    float sea_wave2 = pow(
        1.0 - abs(sin(-p.x * 1.4 + p.y * 2.1 + cos(p.y * 2.0 - u_time * 0.09) * 0.6 + u_time * 0.08)),
        6.0
    );
    float astral_currents = sea_wave1 * 0.65 + sea_wave2 * 0.55;

    float ether_threads = 1.0 - smoothstep(
        0.015,
        0.068,
        abs(sin(warp_b.x * 5.2 + warp_b.y * 4.1 + u_time * 0.065))
    );

    // -------------------------------------------------------------------------
    // Starfield & Scintillating Celestial Glints
    // -------------------------------------------------------------------------
    // Micro background starfield:
    vec2 star_grid1 = p * 16.0;
    vec2 cell_id1 = floor(star_grid1);
    vec2 cell_uv1 = fract(star_grid1) - 0.5;
    float star_rand1 = hash21(cell_id1 + 12.3);
    float star_twinkle1 = 0.4 + 0.6 * sin(u_time * (1.5 + star_rand1 * 4.0) + star_rand1 * 6.28);
    float star1 = step(0.86, star_rand1) * (1.0 - smoothstep(0.02, 0.18, length(cell_uv1))) * star_twinkle1;

    // Bright astral bodies with 4-point cross diffraction spikes:
    vec2 star_grid2 = p * 7.0 + vec2(3.1, 5.7);
    vec2 cell_id2 = floor(star_grid2);
    vec2 cell_uv2 = fract(star_grid2) - 0.5;
    float star_rand2 = hash21(cell_id2 + 45.6);
    float star_twinkle2 = 0.5 + 0.5 * sin(u_time * (1.2 + star_rand2 * 3.0) + star_rand2 * 6.28);
    float star_core2 = 1.0 - smoothstep(0.01, 0.12, length(cell_uv2));
    float spike_h = (1.0 - smoothstep(0.006, 0.035, abs(cell_uv2.y))) * (1.0 - smoothstep(0.04, 0.35, abs(cell_uv2.x)));
    float spike_v = (1.0 - smoothstep(0.006, 0.035, abs(cell_uv2.x))) * (1.0 - smoothstep(0.04, 0.35, abs(cell_uv2.y)));
    float star2 = step(0.91, star_rand2) * max(star_core2, max(spike_h, spike_v) * 0.85) * star_twinkle2;

    // Drifting stardust motes:
    vec2 dust_grid = p * 11.0 + vec2(sin(u_time * 0.05) * 0.5, u_time * 0.035);
    float dust_rand = hash21(floor(dust_grid) + 89.1);
    float dust_glow = step(0.80, dust_rand)
        * (1.0 - smoothstep(0.02, 0.25, length(fract(dust_grid) - 0.5)))
        * (0.3 + 0.7 * sin(u_time * 2.2 + dust_rand * 6.28));

    // -------------------------------------------------------------------------
    // Spectral Color Grading & Composition
    // -------------------------------------------------------------------------
    // Deep cosmic abyss base:
    vec3 void_bottom = vec3(0.018, 0.015, 0.042);
    vec3 void_top = vec3(0.028, 0.042, 0.088);
    vec3 color = mix(void_bottom, void_top, clamp(p.y * 0.5 + 0.5, 0.0, 1.0));

    // Deep cosmic violet / purple nebula mass:
    color += vec3(0.36, 0.07, 0.56) * pow(nebula_violet, 1.8) * 0.95;

    // Luminous celestial orchid / magenta clouds:
    color += vec3(0.72, 0.12, 0.58) * pow(nebula_magenta, 2.2) * 1.15;
    color += vec3(0.96, 0.28, 0.72) * pow(nebula_magenta * nebula_cyan, 2.8) * 0.88;

    // Astral sea cyan / azure ether streams:
    color += vec3(0.04, 0.58, 0.82) * pow(nebula_cyan, 2.0) * 1.05;
    color += vec3(0.18, 0.85, 0.90) * astral_currents * 0.52;
    color += vec3(0.42, 0.92, 0.96) * ether_threads * 0.38;

    // Starlight gold dust and highlights:
    color += vec3(0.96, 0.70, 0.24) * pow(nebula_magenta * nebula_violet, 3.2) * 0.72;
    color += vec3(0.94, 0.86, 0.62) * dust_glow * 0.48;

    // Twinkling stars:
    color += vec3(0.82, 0.90, 1.0) * star1 * 0.75;
    color += mix(vec3(0.88, 0.94, 1.0), vec3(1.0, 0.82, 0.42), star_rand2) * star2 * 1.25;

    // Deep cosmic vignette to frame the playfield:
    vec2 norm_p = vec2(p.x / u_aspect, p.y);
    float vignette = 1.0 - smoothstep(0.68, 1.42, length(norm_p * vec2(1.05, 1.0)));
    color *= 0.62 + 0.38 * vignette;

    vec4 output_color = vec4(color * v_color.rgb, v_color.a);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
