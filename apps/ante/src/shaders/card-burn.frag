precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_progress;
uniform float u_seed;
uniform float u_aspect;
uniform float u_particle_mode;
uniform vec3 u_accent_color;

// Value noise is deliberately evaluated in card UV space rather than screen
// space. The burn therefore stays attached to a tilted card, while four
// octaves keep the edge from revealing a repeated geometric primitive.
float hash21(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32 + u_seed * 0.013);
    return fract(p.x * p.y);
}

float noise(vec2 p) {
    vec2 cell = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash21(cell), hash21(cell + vec2(1.0, 0.0)), f.x),
        mix(hash21(cell + vec2(0.0, 1.0)), hash21(cell + vec2(1.0)), f.x),
        f.y
    );
}

float fbm(vec2 p) {
    float value = 0.0;
    float amplitude = 0.52;
    for (int octave = 0; octave < 4; octave++) {
        value += noise(p) * amplitude;
        p = p * 2.03 + vec2(17.1, 9.2);
        amplitude *= 0.49;
    }
    return value;
}

void main() {
    if (u_particle_mode > 0.5) {
        // Burn particles are emitted as cheap quads, but this branch removes
        // every geometric edge. Multi-scale noise cuts each quad into a soft,
        // torn fleck; elongated quads become embers without revealing their
        // underlying rectangle.
        vec2 p = v_uv * 2.0 - 1.0;
        float angle = atan(p.y, p.x);
        float body_noise = fbm(
            (p + vec2(1.7, 2.3)) * 3.2
                + vec2(u_seed * 0.091, u_seed * 0.047));
        float fine_noise = noise(
            p * 9.0 + vec2(u_seed * 0.13, -u_seed * 0.08));
        float radius = length(p * vec2(0.91, 1.08));
        float boundary = 0.73
            + (body_noise - 0.50) * 0.34
            + (fine_noise - 0.50) * 0.10
            + sin(angle * 5.0 + body_noise * 4.0) * 0.055;
        float particle_alpha = 1.0 - smoothstep(
            boundary - 0.20, boundary + 0.035, radius);
        float porous = smoothstep(0.15, 0.55, fine_noise + particle_alpha);
        particle_alpha *= porous;

        float ember_kind = step(0.5, u_progress);
        float ember_core = exp(-radius * radius * 3.8);
        vec3 particle_color = v_color.rgb;
        particle_color *= 0.72 + body_noise * 0.42;
        particle_color += vec3(0.32, 0.055, 0.008)
            * ember_core * ember_kind;
        particle_alpha *= mix(0.82, 1.0, ember_kind * ember_core);
        particle_alpha *= v_color.a;
        if (particle_alpha < 0.018) discard;
        gl_FragColor = vec4(particle_color, particle_alpha);
        return;
    }

    vec4 card = texture2D(u_texture, v_uv) * v_color;
    if (card.a < 0.01) discard;

    // Render-target capture clips a stroke centred directly on its outer
    // bounds, and the very narrow school stripe is vulnerable to filtering.
    // Restore those two pieces from their canonical card-space geometry before
    // combustion. They still pass through the same char and alpha fronts below,
    // so this preserves intact identity without painting over burned pixels.
    float card_edge_distance = min(
        min(v_uv.x, 1.0 - v_uv.x), min(v_uv.y, 1.0 - v_uv.y));
    float frame_mask = 1.0 - smoothstep(0.007, 0.022, card_edge_distance);
    float stripe_x = smoothstep(0.055, 0.069, v_uv.x)
        * (1.0 - smoothstep(0.111, 0.124, v_uv.x));
    float stripe_y = smoothstep(0.043, 0.058, v_uv.y)
        * (1.0 - smoothstep(0.942, 0.957, v_uv.y));
    float stripe_mask = stripe_x * stripe_y;
    card.rgb = mix(card.rgb, vec3(0.32, 0.32, 0.36), frame_mask);
    card.rgb = mix(card.rgb, u_accent_color, stripe_mask);

    vec2 from_center = (v_uv - 0.5) * vec2(u_aspect, 1.0);
    float corner = 0.5 * length(vec2(u_aspect, 1.0));
    float radial = length(from_center) / corner;
    float angle = atan(from_center.y, from_center.x);

    vec2 grain_uv = v_uv * vec2(8.0, 10.0)
        + vec2(u_seed * 0.071, u_seed * 0.037);
    float broad = fbm(grain_uv);
    float fibers = noise(v_uv * vec2(39.0, 57.0) + u_seed);
    float edge_field = radial
        + (broad - 0.50) * 0.23
        + (fibers - 0.50) * 0.045
        + sin(angle * 7.0 + broad * 5.0) * 0.018;

    // The scorch stays only a narrow step ahead of the hole. Both fronts begin
    // below the minimum noisy centre value, so the first frames preserve the
    // sampled card exactly—including its school stripe—before a pinprick
    // ignition appears. They then cross every pixel in the same local order:
    // intact art, browned paper, hot cut edge, transparency.
    float char_t = smoothstep(0.06, 0.94, u_progress);
    // The hole trails the scorch rather than waiting for the final frames.
    // Keeping both fronts visible at once is what makes this read as paper
    // actively being eaten: black material outside, real transparency inside,
    // and the emissive boundary shared by both.
    float hole_t = smoothstep(0.11, 0.97, u_progress);
    float char_front = -0.20 + char_t * 1.48;
    float hole_front = -0.24 + hole_t * 1.54;
    float charred = 1.0 - smoothstep(char_front - 0.025, char_front + 0.025, edge_field);
    float burned_away = 1.0 - smoothstep(hole_front - 0.018, hole_front + 0.018, edge_field);

    // Paper fibers catch and cool at different rates. This breaks the black
    // region into warm brown fissures and cold grey islands before it falls.
    float coal_noise = fbm(grain_uv * 1.7 + vec2(5.2, -3.8));
    vec3 cold_ash = mix(
        vec3(0.105, 0.078, 0.062),
        vec3(0.34, 0.26, 0.20),
        coal_noise);
    vec3 color = mix(card.rgb, cold_ash, charred * 0.70);
    color += vec3(0.060, 0.042, 0.028)
        * smoothstep(0.02, 0.16, u_progress)
        * (1.0 - burned_away)
        * (1.0 - charred * 0.45);

    // This is the actual cut edge of the card, not a second ring drawn over
    // it. The glow therefore inherits the same noisy boundary as alpha and is
    // automatically clipped by the surviving paper all the way to the four
    // corners. A tight white-hot core sits inside a broader orange ember band.
    float ember_edge = exp(-abs(edge_field - hole_front) * 43.0)
        * smoothstep(0.11, 0.29, u_progress);
    float ember_band = exp(-abs(edge_field - hole_front) * 19.0)
        * smoothstep(0.14, 0.32, u_progress);
    float ignition_edge = exp(-abs(edge_field - char_front) * 42.0)
        * (1.0 - smoothstep(0.50, 0.72, u_progress));
    float flicker = 0.76
        + 0.16 * sin(u_time * 18.0 + angle * 5.0 + broad * 8.0)
        + 0.08 * sin(u_time * 31.0 - angle * 3.0);
    float heat = max(max(ember_edge, ember_band * 0.46), ignition_edge * 0.68)
        * flicker;
    vec3 ember = mix(vec3(0.95, 0.12, 0.015), vec3(1.0, 0.76, 0.16), clamp(heat * 1.8, 0.0, 1.0));
    color = mix(color, ember, clamp(heat * 1.62, 0.0, 1.0));
    color += vec3(0.24, 0.035, 0.003) * ember_band * flicker;

    float paper_edge_distance = min(
        min(v_uv.x, 1.0 - v_uv.x), min(v_uv.y, 1.0 - v_uv.y));
    float exposed_edge = 1.0 - smoothstep(0.008, 0.032, paper_edge_distance);
    color = mix(color, vec3(0.32, 0.27, 0.23), exposed_edge * charred * 0.72);
    color = mix(
        color, vec3(0.30, 0.12, 0.045),
        exposed_edge * smoothstep(0.12, 0.46, u_progress)
            * (1.0 - burned_away) * 0.58);

    // A sparse constellation of coals remains in the black stage instead of
    // reading as one flat mask. It disappears as the hole reaches each fiber.
    float coal = smoothstep(0.78, 0.94, fibers)
        * charred * (1.0 - burned_away)
        * smoothstep(0.22, 0.43, u_progress);
    color += vec3(0.68, 0.10, 0.015) * coal * flicker;

    float alpha = card.a * (1.0 - smoothstep(0.72, 0.995, burned_away));
    alpha *= 1.0 - smoothstep(0.90, 1.0, u_progress) * smoothstep(0.82, 1.08, edge_field);
    if (alpha < 0.012) discard;
    gl_FragColor = vec4(color, alpha);
}
