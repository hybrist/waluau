precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_aspect;
uniform float u_freeze;
uniform vec4 u_rect;

// u_texture is assets/ice-noise.png, a tileable data texture:
//   r frost grain fbm, g voronoi ridge (crack) field,
//   b per-chunk facet value, a high-frequency sparkle noise.
vec4 ice_noise(vec2 p) {
    return texture2D(u_texture, fract(p));
}

float tooth_profile(float f) {
    float w = 1.0 - abs(2.0 * f - 1.0);
    return pow(w, 0.65);
}

// Icicles hang in the quad margin outside one card edge. `along` runs along
// the edge in aspect-true units, `drop` grows away from the card face.
vec2 icicle(float along, float drop, float grow, float count, float reach) {
    float slot = floor(along * count);
    vec4 slot_noise = ice_noise(vec2((slot + 0.5) / count, 0.31 * slot));
    float length_ = reach * grow * (0.30 + 0.70 * slot_noise.b);
    float profile = tooth_profile(fract(along * count));
    float edge = length_ * profile;
    float body = 1.0 - smoothstep(edge - 0.012, edge, drop);
    if (body <= 0.0) return vec2(0.0);
    float core = smoothstep(0.0, 0.35, profile) * body;
    float tip = smoothstep(0.55, 1.0, drop / max(edge, 0.001));
    return vec2(core, tip * core);
}

void main() {
    // Card-local coordinates: 0..1 inside the card face, outside in margins.
    vec2 span = u_rect.zw - u_rect.xy;
    vec2 p = (v_uv - u_rect.xy) / span;
    vec2 q = vec2(p.x * u_aspect, p.y);

    vec3 deep_blue = vec3(0.286, 0.671, 0.898);   // #49abe5
    vec3 shell_blue = vec3(0.639, 0.867, 0.973);  // #a3ddf8
    vec3 frost_white = vec3(0.937, 0.976, 1.0);   // #eff9ff

    float freeze = clamp(u_freeze, 0.0, 1.0);
    vec3 color = vec3(0.0);
    float alpha = 0.0;

    if (p.x > 0.0 && p.x < 1.0 && p.y > 0.0 && p.y < 1.0) {
        // Distance from the nearest card edge; frost creeps toward the centre
        // and the corners lead because they touch two edges at once.
        float edge_distance = min(min(q.x, u_aspect - q.x), min(q.y, 1.0 - q.y));
        vec4 warp = ice_noise(p * 1.31 + vec2(0.17, 0.43));
        float front = freeze * (0.5 * u_aspect + 0.16);
        float creep = edge_distance + (warp.r - 0.5) * 0.17;
        float coverage = smoothstep(front, front - 0.11, creep);

        vec4 chunk = ice_noise(q * 0.52 + vec2(0.05, 0.61));
        vec4 grain_a = ice_noise(q * 1.9 + vec2(0.71, 0.13));
        vec4 grain_b = ice_noise(q * 4.3 + vec2(0.29, 0.83));
        float grain = grain_a.r * 0.62 + grain_b.r * 0.38;

        // Faceted chunks: flat per-cell shading broken by thin crack ridges.
        // A second, finer crack net keeps large plates from looking blank.
        float crack = 1.0 - smoothstep(0.015, 0.10, chunk.g);
        vec4 chunk_fine = ice_noise(q * 1.15 + vec2(0.42, 0.17));
        float fine_crack = 1.0 - smoothstep(0.01, 0.07, chunk_fine.g);
        float facet = 0.72 + 0.56 * (chunk.b - 0.5);
        float rim = 1.0 - smoothstep(0.0, 0.13, edge_distance);
        float rim_frost = rim * rim * (0.55 + 0.45 * grain);

        // The crystallization front glows while it advances.
        float front_line = coverage
            * (1.0 - smoothstep(0.0, 0.085, abs(creep - front + 0.055)));

        // Two cool highlights breathe across the shell.
        float breathe = 0.5 + 0.5 * sin(u_time * 1.7 + q.x * 2.6 + q.y * 1.9);
        float sweep = 0.5 + 0.5 * sin(u_time * 0.9 - q.x * 3.8 + q.y * 4.4);
        vec3 shell = mix(deep_blue, shell_blue, facet * (0.30 + 0.30 * breathe));
        shell = mix(shell, frost_white,
            rim_frost * 0.85 + crack * 0.32 + fine_crack * 0.12);
        shell += frost_white * front_line * 0.9;
        shell += shell_blue * sweep * 0.12;

        // Sparkle: scattered glints that twinkle out of phase.
        float twinkle = 0.5 + 0.5 * sin(u_time * 3.1 + grain_b.a * 39.0);
        float sparkle = smoothstep(0.965, 0.995, grain_b.a * (0.82 + 0.18 * twinkle));
        shell += frost_white * sparkle * 0.9;

        // The centre stays a thinner glaze so the card face reads through it;
        // frost thickens toward the rim, along cracks, and on bright facets.
        float body_alpha = 0.18 + 0.12 * grain + 0.08 * facet;
        body_alpha = mix(body_alpha, 0.85, rim_frost);
        body_alpha += crack * 0.38 + fine_crack * 0.10
            + front_line * 0.55 + sparkle * 0.6;
        color = shell;
        alpha = coverage * clamp(body_alpha, 0.0, 0.9);
    } else {
        // Margins: icicles below, stubby frost teeth above, faint halo beside.
        float settle = smoothstep(0.45, 1.0, freeze);
        if (p.y >= 1.0 && p.x > 0.0 && p.x < 1.0) {
            // A frozen lip runs along the card's bottom edge; icicles hang
            // from it so they read as growth rather than floating teeth.
            float drop = p.y - 1.0;
            vec2 spike = icicle(q.x + 0.013, drop, settle, 8.0, 0.235);
            float lip = (1.0 - smoothstep(0.012, 0.035, drop)) * settle;
            float melt_glint = 0.5 + 0.5 * sin(u_time * 2.3 + q.x * 21.0);
            color = mix(shell_blue, frost_white,
                max(spike.y, lip * 0.55) + melt_glint * 0.18);
            alpha = max(spike.x * 0.92, lip * 0.8);
        } else if (p.y <= 0.0 && p.x > 0.0 && p.x < 1.0) {
            // Two tooth pitches overlap so the rime ridge stays irregular.
            vec2 spike = icicle(q.x + 0.47, -p.y, settle, 23.0, 0.055);
            vec2 wide = icicle(q.x + 0.19, -p.y, settle, 11.0, 0.042);
            spike = max(spike, wide * 0.85);
            color = mix(shell_blue, frost_white, spike.y);
            alpha = spike.x * 0.7;
        } else {
            float outside = max(max(-p.x, p.x - 1.0) * u_aspect, max(-p.y, p.y - 1.0));
            float halo = (1.0 - smoothstep(0.0, 0.05, outside)) * settle;
            color = shell_blue;
            alpha = halo * 0.28;
        }
    }

    gl_FragColor = vec4(color, alpha) * v_color;
}
