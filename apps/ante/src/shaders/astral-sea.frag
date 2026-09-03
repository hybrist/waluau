precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_aspect;

// The Astral Sea: a living cosmic expanse behind the duel board.
// Billowing multi-layered nebulae in imperial violet, ethereal magenta, and
// radiant celestial cyan drift across deep cosmic obsidian, studded with
// twinkling stardust and luminous energy currents.

float hash21(vec2 p) {
    p = fract(p * vec2(234.34, 435.345));
    p += dot(p, p + 34.23);
    return fract(p.x * p.y);
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

// 5-octave FBM for rich, organic cloud textures.
float fbm(vec2 p) {
    float total = 0.0;
    float amp = 0.5;
    mat2 rot = mat2(0.80, 0.60, -0.60, 0.80);
    for (int i = 0; i < 5; ++i) {
        total += amp * noise(p);
        p = rot * p * 2.02 + vec2(1.7, 9.2);
        amp *= 0.5;
    }
    return total;
}

void main() {
    // Correct coordinates to preserve cosmic aspect ratio across widescreen and mobile
    vec2 p = vec2((v_uv.x - 0.5) * u_aspect, v_uv.y - 0.5) * 1.85;

    // Slow, majestic astral time scales
    float t = u_time * 0.024;

    // Double domain warping: creates organic, turbulent, fluid-like nebula ribbons
    vec2 q = vec2(
        fbm(p + vec2(0.0, 0.0) + t),
        fbm(p + vec2(4.2, 1.7) - t * 0.85)
    );

    vec2 r = vec2(
        fbm(p + 3.2 * q + vec2(1.7, 8.4) + t * 1.25),
        fbm(p + 3.2 * q + vec2(7.3, 2.6) - t * 1.10)
    );

    float f = fbm(p + 3.6 * r);

    // Deep cosmic space baseline: midnight obsidian with violet undertone
    vec3 deep_space = vec3(0.022, 0.012, 0.048);
    vec3 cosmic_blue = vec3(0.035, 0.045, 0.12);
    vec3 base_void = mix(deep_space, cosmic_blue, v_uv.y * 0.65 + 0.35 * fbm(p * 0.4));

    // Vibrant nebula palettes:
    // Imperial purple / violet
    vec3 nebula_purple = vec3(0.46, 0.09, 0.64);
    vec3 nebula_violet = vec3(0.68, 0.18, 0.88);
    // Celestial cyan / radiant azure
    vec3 nebula_azure = vec3(0.05, 0.38, 0.74);
    vec3 nebula_cyan = vec3(0.12, 0.76, 0.86);
    // Arcane rose / magenta filament accents
    vec3 nebula_rose = vec3(0.86, 0.22, 0.64);
    // Hot celestial starlight / core energy
    vec3 core_white = vec3(0.96, 0.94, 1.0);

    // Composite nebula color layers based on warp coordinates
    vec3 color = base_void;

    // Layer 1: Broad billowing violet clouds
    float cloud_a = smoothstep(0.18, 0.72, f);
    vec3 col_a = mix(nebula_purple, nebula_violet, clamp(length(q), 0.0, 1.0));
    color += col_a * cloud_a * 0.82;

    // Layer 2: Radiant cyan/azure astral currents weaving through the void
    float cloud_b = smoothstep(0.28, 0.85, length(r));
    vec3 col_b = mix(nebula_azure, nebula_cyan, clamp(r.x * 1.4, 0.0, 1.0));
    color += col_b * cloud_b * 0.72;

    // Layer 3: Intense arcane rose filaments at high warp density
    float filaments = smoothstep(0.55, 0.88, f * length(q));
    color = mix(color, nebula_rose, filaments * 0.58);

    // Layer 4: Luminous core glow in the brightest pockets
    float core = smoothstep(0.72, 0.96, f * f + cloud_b * 0.45);
    color += core_white * core * 0.42;

    // Subtle, slow breathing pulsation across the nebulae
    float pulse = 0.94 + 0.06 * sin(u_time * 0.42 + p.x * 1.8 + p.y * 1.2);
    color *= pulse;

    // --- Starfields & Celestial Dust (smooth circular pinpricks & halos) ---
    // 1. Fine twinkling stardust
    vec2 dust_uv = p * 28.0;
    vec2 dust_id = floor(dust_uv);
    vec2 dust_f = fract(dust_uv) - 0.5;
    float dust_rnd = hash21(dust_id);
    if (dust_rnd > 0.92) {
        vec2 dust_pos = (vec2(hash21(dust_id + 0.2), hash21(dust_id + 0.8)) - 0.5) * 0.6;
        float d = length(dust_f - dust_pos);
        float twinkle = 0.6 + 0.4 * sin(u_time * (2.0 + dust_rnd * 4.0) + dust_rnd * 6.28);
        float pt = smoothstep(0.12, 0.0, d);
        color += vec3(0.85, 0.90, 1.0) * pt * twinkle * 0.75;
    }

    // 2. Distinct brighter stars with soft halos
    vec2 star_uv = p * 9.0;
    vec2 star_id = floor(star_uv);
    vec2 star_f = fract(star_uv) - 0.5;
    float star_rnd = hash21(star_id + 53.1);
    if (star_rnd > 0.88) {
        vec2 star_pos = (vec2(hash21(star_id + 0.35), hash21(star_id + 0.75)) - 0.5) * 0.6;
        float d = length(star_f - star_pos);
        float twinkle = 0.65 + 0.35 * sin(u_time * (1.5 + star_rnd * 3.0) + star_rnd * 6.28);
        float star_core = smoothstep(0.08, 0.0, d);
        float star_halo = smoothstep(0.35, 0.0, d) * 0.35;
        vec3 star_color = mix(vec3(0.82, 0.92, 1.0), vec3(1.0, 0.85, 0.65), fract(star_rnd * 7.1));
        color += star_color * (star_core + star_halo) * twinkle * 1.1;
    }

    // --- Vignette & Duel Board Framing ---
    // Subtle falloff at the perimeter closes to deep obsidian to keep card contrast sharp
    vec2 corner_dist = abs(v_uv - 0.5) * 2.0;
    float edge_falloff = 1.0 - smoothstep(0.55, 1.32, length(corner_dist));
    color = mix(vec3(0.015, 0.008, 0.035), color, edge_falloff);

    vec4 output_color = vec4(color, 1.0);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
