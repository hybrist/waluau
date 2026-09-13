// The city seen through a scrying spell. The picture underneath is an ordinary
// plan of the streets, captured into a texture; this program is the water it
// is seen in. The centre of the pool is still and sharp. Outward from it the
// image turns about the focus, smears along that turn, splits into its
// colours and cools toward violet, until the rim: a thin bright ring of the
// spell's own light with the void beyond it.
//
// highp for the same reason as the astral sea: the shimmer is hashed noise
// over large coordinates, which breaks into blocks at half precision.
precision highp float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform sampler2D u_texture;
// Width over height of the quad, so the pool is round on any canvas.
uniform float u_aspect;
// Seconds of the spell. Passed in rather than read from u_time so a viewer
// who prefers reduced motion can be shown one fixed moment.
uniform float u_phase;
// How far the picture turns at the rim, how far it smears along that turn,
// how much the surface ripples, each 0..1.
uniform float u_swirl;
uniform float u_blur;
uniform float u_ripple;
// Radius of the pool in half-heights of the canvas: 1.0 touches top and bottom.
uniform float u_aperture;

const vec3 VOID = vec3(0.012, 0.008, 0.030);
const vec3 SPELL = vec3(0.36, 0.14, 0.62);
const vec3 SPELL_BRIGHT = vec3(0.62, 0.90, 1.00);
const vec3 WATER = vec3(0.60, 0.72, 1.00);
const int TAPS = 6;

float hash21(vec2 p) {
    p = fract(p * vec2(233.34, 851.73));
    p += dot(p, p + 23.45);
    return fract(p.x * p.y);
}

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

// From the pool's own frame, centred with y in -1..1, back to a texture read.
vec2 to_uv(vec2 centred) {
    return centred / (vec2(u_aspect, 1.0) * 2.0) + 0.5;
}

void main() {
    vec2 centred = (v_uv - 0.5) * vec2(u_aspect, 1.0) * 2.0;
    float radius = length(centred);
    // 0 at the focus, 1 at the rim of the pool.
    float pool = radius / u_aperture;
    float angle = atan(centred.y, centred.x);

    // The turn: nothing at the focus, most at the rim, and breathing so the
    // picture is never at rest. The rim of a pool of water rotating a picture
    // is the one thing that says "seen in water" from across the room.
    float breath = 0.6 + 0.4 * sin(u_phase * 0.55);
    float turn = u_swirl * 1.25 * pool * pool * breath;

    // Rings running in toward the focus, and a slower cross-wave so they never
    // read as a stationary target. Both move the sample along its radius.
    float rings = sin(pool * 34.0 - u_phase * 2.4)
        + 0.55 * sin(pool * 57.0 + u_phase * 1.5 + angle * 3.0);
    float ripple = u_ripple * 0.010 * rings * smoothstep(0.12, 0.85, pool);

    float bent = angle + turn;
    float bent_radius = radius + ripple;

    // The smear runs along the turn — an arc, not a straight blur — and grows
    // toward the rim, where the water is moving. The taps also spread a little
    // in radius so the arc does not read as a clean line.
    float arc = u_blur * 0.11 * smoothstep(0.15, 1.0, pool);
    vec3 color = vec3(0.0);
    float total = 0.0;
    for (int tap = -TAPS; tap <= TAPS; tap++) {
        float t = float(tap) / float(TAPS);
        float a = bent + t * arc;
        float r = bent_radius * (1.0 + t * arc * 0.30);
        float weight = 1.0 - abs(t) * 0.55;
        color += texture2D(u_texture, to_uv(vec2(cos(a), sin(a)) * r)).rgb * weight;
        total += weight;
    }
    color /= total;

    // Colour fringing at the rim: red read a little farther out than blue.
    vec2 uv = to_uv(vec2(cos(bent), sin(bent)) * bent_radius);
    float fringe = 0.012 * pool * pool * (0.4 + u_blur);
    vec3 split = vec3(
        texture2D(u_texture, mix(vec2(0.5), uv, 1.0 + fringe)).r,
        color.g,
        texture2D(u_texture, mix(vec2(0.5), uv, 1.0 - fringe)).b
    );
    color = mix(color, split, smoothstep(0.35, 1.0, pool));

    // The water's tint: the plan keeps its own colours at the focus and cools
    // toward the rim, where light is moving across the surface.
    float luma = dot(color, vec3(0.299, 0.587, 0.114));
    color = mix(color, luma * WATER, smoothstep(0.40, 1.0, pool) * 0.65);
    float shimmer = noise(centred * 5.0 + vec2(u_phase * 0.35, -u_phase * 0.2))
        * noise(centred * 11.0 - vec2(u_phase * 0.25, u_phase * 0.45));
    color *= 0.94 + 0.30 * shimmer * smoothstep(0.20, 0.85, pool);
    color *= 1.0 - 0.50 * smoothstep(0.50, 1.0, pool);

    // The rim: a hard edge to the water, a thin line of the spell's light on
    // it, a wider glow both ways from it, and faint slow rings out in the
    // void as the spell's light spreads past the pool.
    float inside = 1.0 - smoothstep(0.975, 1.0, pool);
    float distance_to_rim = pool - 1.0;
    float line = exp(-distance_to_rim * distance_to_rim * 900.0);
    float glow = exp(-distance_to_rim * distance_to_rim * 22.0);
    float flicker = 0.85 + 0.15 * sin(u_phase * 1.7 + angle * 2.0);
    vec3 out_color = mix(VOID, color, inside);
    float outside = step(0.0, distance_to_rim);
    float halo_rings = 0.5 + 0.5 * sin(pool * 36.0 - u_phase * 1.1);
    out_color += SPELL * outside * halo_rings * 0.10 * exp(-distance_to_rim * 3.5);
    out_color += mix(SPELL, SPELL_BRIGHT, line) * (line * 1.1 + glow * 0.55) * flicker;

    gl_FragColor = vec4(out_color, v_color.a);
}
