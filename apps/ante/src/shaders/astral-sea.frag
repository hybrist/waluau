precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_aspect;

float hash21(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

float value_noise(vec2 p) {
    vec2 cell = floor(p);
    vec2 part = fract(p);
    part = part * part * (3.0 - 2.0 * part);
    float a = hash21(cell);
    float b = hash21(cell + vec2(1.0, 0.0));
    float c = hash21(cell + vec2(0.0, 1.0));
    float d = hash21(cell + vec2(1.0, 1.0));
    return mix(mix(a, b, part.x), mix(c, d, part.x), part.y);
}

float fbm(vec2 p) {
    float value = 0.0;
    float weight = 0.52;
    value += value_noise(p) * weight;
    p = mat2(0.80, -0.60, 0.60, 0.80) * p * 2.03 + 9.7;
    weight *= 0.50;
    value += value_noise(p) * weight;
    p = mat2(0.80, -0.60, 0.60, 0.80) * p * 2.01 + 9.7;
    weight *= 0.50;
    value += value_noise(p) * weight;
    p = mat2(0.80, -0.60, 0.60, 0.80) * p * 2.04 + 9.7;
    weight *= 0.50;
    value += value_noise(p) * weight;
    p = mat2(0.80, -0.60, 0.60, 0.80) * p * 2.02 + 9.7;
    weight *= 0.50;
    value += value_noise(p) * weight;
    return value;
}

void main() {
    vec2 centred = (v_uv - 0.5) * vec2(u_aspect, 1.0);
    float drift = u_time * 0.024;

    // Two immense, opposing currents warp one another. Their motion is slow
    // enough to feel celestial rather than busy while their saturated violet,
    // cyan, and rose make the sea read boldly behind the dark play surface.
    vec2 domain = centred * 1.16;
    float fold_a = fbm(domain * 1.15 + vec2(drift, -drift * 0.41));
    float fold_b = fbm(
        domain * 1.42
            + vec2(-drift * 0.36, drift * 0.29)
            + vec2(fold_a * 1.34, -fold_a * 0.91)
    );
    vec2 warped = domain + vec2(fold_a - 0.48, fold_b - 0.48) * 0.72;
    float body = fbm(warped * 1.48 + vec2(-drift * 0.18, drift * 0.22));
    float inner = fbm(warped * 2.71 + vec2(drift * 0.31, drift * 0.12));
    float cloud = smoothstep(0.30, 0.76, body + fold_b * 0.27);
    float heart = smoothstep(0.47, 0.84, inner + body * 0.31);
    float filament = (1.0 - smoothstep(0.0, 0.035, abs(inner - 0.53))) * cloud;

    vec3 midnight = vec3(0.008, 0.010, 0.048);
    vec3 indigo = vec3(0.105, 0.055, 0.34);
    vec3 amethyst = vec3(0.40, 0.085, 0.50);
    vec3 astral_blue = vec3(0.025, 0.36, 0.55);
    vec3 rose_magic = vec3(0.58, 0.08, 0.34);
    vec3 color = mix(midnight, indigo, cloud * 0.74);
    color += amethyst * heart * 0.30;
    color += astral_blue * smoothstep(0.46, 0.82, fold_a + inner * 0.23) * 0.24;
    color += rose_magic * filament * 0.22;

    // A sparse fixed star field carries a restrained magical twinkle. It is
    // strongest away from the table so no flashing point competes with ranks,
    // suits, or prompts in the centre of play.
    vec2 star_space = centred * 92.0;
    vec2 star_cell = floor(star_space);
    vec2 star_part = fract(star_space) - 0.5;
    float star_seed = hash21(star_cell + 37.0);
    float star_gate = smoothstep(0.986, 0.999, star_seed);
    float star_shape = 1.0 - smoothstep(0.025, 0.105, length(star_part));
    float centre_distance = length(centred / vec2(max(u_aspect * 0.5, 0.5), 0.62));
    float star_edge = mix(0.30, 1.0, smoothstep(0.25, 0.90, centre_distance));
    float twinkle = 0.72 + 0.28 * sin(u_time * 0.72 + star_seed * 31.0);
    vec3 starlight = mix(vec3(0.50, 0.73, 1.0), vec3(1.0, 0.70, 0.93), star_seed);
    color += starlight * star_gate * star_shape * star_edge * twinkle * 0.58;

    // A dark glass veil beneath the card formation protects foreground
    // contrast; the vignette then gives the infinite field a strong frame.
    float table = 1.0 - smoothstep(0.30, 1.02, length(centred / vec2(0.84, 0.64)));
    color *= 1.0 - table * 0.31;
    vec2 edge_uv = abs(v_uv - 0.5) * 2.0;
    float vignette = smoothstep(0.46, 1.0, max(edge_uv.x, edge_uv.y));
    color *= 1.0 - vignette * 0.57;
    color *= 0.92 + 0.08 * smoothstep(0.0, 0.8, 1.0 - abs(v_uv.y - 0.54));

    vec4 output_color = vec4(color, 1.0) * v_color;
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
