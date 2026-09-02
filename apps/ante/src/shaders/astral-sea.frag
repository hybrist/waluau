// The astral sea the vault opens onto: nebulas drifting behind the table and
// a field of stars turning slowly through them. Every colour is kept under
// the board's ink — the cards and the HUD read over the sea, so even the
// densest core of a nebula sits well below a card face.
//
// highp rather than the mediump the card effects use: the noise below is
// built from hashes of large coordinates, and at half precision those hashes
// break into visible blocks on mobile GPUs.
precision highp float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_aspect;
// Seconds of drift. The board passes this in rather than reading u_time so a
// viewer who has asked for reduced motion can be shown one fixed moment.
uniform float u_phase;

const vec3 DEEP = vec3(0.020, 0.012, 0.055);
const vec3 VIOLET = vec3(0.24, 0.08, 0.46);
const vec3 MAGENTA = vec3(0.74, 0.16, 0.58);
const vec3 TEAL = vec3(0.08, 0.48, 0.64);
const vec3 EMBER = vec3(0.96, 0.58, 0.32);
const vec3 STARLIGHT = vec3(0.86, 0.90, 1.00);
const mat2 TURN = mat2(0.80, 0.60, -0.60, 0.80);

float hash21(vec2 p) {
    p = fract(p * vec2(233.34, 851.73));
    p += dot(p, p + 23.45);
    return fract(p.x * p.y);
}

// Value noise: the hashes at a cell's four corners, blended with a smooth
// step so the cell edges do not show as creases.
float noise(vec2 p) {
    vec2 cell = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash21(cell), hash21(cell + vec2(1.0, 0.0)), u.x),
        mix(hash21(cell + vec2(0.0, 1.0)), hash21(cell + vec2(1.0, 1.0)), u.x),
        u.y
    );
}

// Four octaves, each turned against the last so the grid never lines up.
float fbm(vec2 p) {
    float value = 0.0;
    float amplitude = 0.5;
    for (int octave = 0; octave < 4; octave++) {
        value += amplitude * noise(p);
        p = TURN * p * 2.02 + 7.3;
        amplitude *= 0.5;
    }
    return value;
}

// One layer of stars: at most one per grid cell, placed by the cell's hash,
// and only in the cells the hash allows so they gather rather than tile. The
// rarest seeds make the largest stars, and those carry a four-point flare.
float star_layer(vec2 p, float fill, float radius, float phase) {
    vec2 cell = floor(p);
    vec2 local = fract(p) - 0.5;
    float seed = hash21(cell);
    if (seed > fill) return 0.0;
    vec2 offset = (vec2(hash21(cell + 17.3), hash21(cell + 41.9)) - 0.5) * 0.7;
    vec2 to_star = local - offset;
    float distance = length(to_star);
    float size = radius * (1.5 - seed / fill);
    float core = 1.0 - smoothstep(0.0, size, distance);
    core *= core;
    float twinkle = 0.65 + 0.35 * sin(phase * (1.1 + seed * 2.5) + seed * 61.0);
    float flare = (1.0 - smoothstep(0.0, size * 0.25, min(abs(to_star.x), abs(to_star.y))))
        * (1.0 - smoothstep(0.0, size * 5.0, distance))
        * step(seed, fill * 0.12);
    return (core + flare * 0.6) * twinkle;
}

void main() {
    vec2 p = (v_uv - 0.5) * vec2(u_aspect, 1.0) * 2.0;
    float drift = u_phase * 0.016;

    // The nebula: noise warped by noise, twice. The warp is where the folds
    // and filaments come from, and the drift moves the warp rather than the
    // noise it warps, which reads as gas turning over instead of a pattern
    // sliding past.
    vec2 field = p * 1.2;
    vec2 q = vec2(
        fbm(field + vec2(drift, drift * 0.4)),
        fbm(field + vec2(5.2, 1.3) - vec2(drift * 0.7, drift * 0.3))
    );
    vec2 r = vec2(
        fbm(field + 3.2 * q + vec2(1.7, 9.2) + drift * 0.3),
        fbm(field + 3.2 * q + vec2(8.3, 2.8) - drift * 0.2)
    );
    float cloud = fbm(field + 2.8 * r);

    // The palette by density: the deep between the clouds, violet gas, a
    // magenta bloom at the dense cores, teal where the second current shows
    // through the thin parts, and an ember glint in the very densest knots.
    float body = smoothstep(0.28, 0.78, cloud);
    float core = smoothstep(0.55, 0.92, cloud);
    vec3 color = mix(DEEP, VIOLET, body);
    color = mix(color, MAGENTA, core * (0.45 + 0.55 * q.x));
    color = mix(
        color,
        TEAL,
        smoothstep(0.35, 0.75, r.y) * (1.0 - body) * 0.75 * smoothstep(0.15, 0.5, cloud)
    );
    color = mix(color, EMBER, smoothstep(0.80, 0.98, cloud) * 0.55);

    // A wide glow above the table's centre, where the densest gas gathers,
    // and a breath over the whole sea slow enough to be felt rather than seen.
    vec2 to_heart = p - vec2(0.0, -0.3);
    float heart = exp(-dot(to_heart, to_heart) * 0.7);
    color += MAGENTA * heart * body * 0.28;
    color *= 0.92 + 0.08 * sin(u_phase * 0.11);

    // Stars in two layers that drift apart, dimmed where the gas is thick.
    float far = star_layer(p * 24.0 + vec2(drift * 0.5, 0.0), 0.32, 0.10, u_phase);
    float near = star_layer(TURN * p * 9.5 + vec2(drift * 1.2, 0.0), 0.16, 0.09, u_phase * 1.3);
    float veil = 1.0 - body * 0.55;
    color += STARLIGHT * (far * 0.55 + near * 0.9) * veil;

    // The walls: black closing in from every edge, measured in the board's
    // own frame so the corners close the same on any aspect.
    vec2 edge = (v_uv - 0.5) * 2.0;
    color *= 1.0 - 0.8 * smoothstep(0.55, 1.5, length(edge));

    gl_FragColor = vec4(color, v_color.a);
}
