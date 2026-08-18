precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_aspect;
uniform float u_selected;
uniform float u_colored;

float rounded_card_distance() {
    float radius = 0.0625;
    vec2 point = vec2(v_uv.x * u_aspect, v_uv.y);
    vec2 half_size = vec2(u_aspect * 0.5, 0.5);
    vec2 q = abs(point - half_size) - (half_size - radius);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

float caustic_ridge(float wave) {
    float distance_to_light = 1.0 - abs(sin(wave));
    return distance_to_light * distance_to_light * distance_to_light
        * distance_to_light * distance_to_light;
}

float swell_ridge(float wave) {
    return 1.0 - smoothstep(0.08, 0.42, abs(sin(wave)));
}

void main() {
    if (rounded_card_distance() > 0.0) discard;
    vec2 p = vec2((v_uv.x - 0.5) * 2.0 * u_aspect, (v_uv.y - 0.5) * 2.0);

    // Slow crossing currents bend the whole light field. The lower frequency
    // keeps the motion legible at card size instead of turning it into noise.
    vec2 q = p;
    q.x += sin(p.y * 2.7 + u_time * 0.34) * 0.120;
    q.x += sin(p.y * 5.9 - u_time * 0.23) * 0.042;
    q.y += sin(p.x * 2.4 - u_time * 0.29) * 0.105;
    q.y += sin(p.x * 5.1 + u_time * 0.21) * 0.038;

    // Broad, mostly horizontal wave fronts travel through the card as one
    // coherent swell. A second offset train breaks the crests into shifting
    // ribbons instead of a stack of parallel stripes.
    float wave1 = swell_ridge(
        q.y * 4.0
        + sin(q.x * 3.2 + u_time * 0.31) * 1.05
        - u_time * 0.52
    );
    float wave2 = swell_ridge(
        q.y * 5.4
        + sin(q.x * 2.2 - u_time * 0.27) * 0.72
        + u_time * 0.38
        + 1.7
    );
    float crest_break = 0.42 + 0.58 * (
        0.5 + 0.5 * sin(q.x * 8.0 - q.y * 2.6 + u_time * 0.44)
    );
    float swells = max(wave1 * crest_break, wave2 * (1.0 - crest_break * 0.35));

    // Finer caustics sit on the swells. Their unequal directions and broken
    // intensity make cells rather than the uniformly bright net this effect
    // used to produce.
    float ridge1 = caustic_ridge(q.x * 6.1 + q.y * 2.5 + u_time * 0.58
        + sin(q.y * 4.2 - u_time * 0.31) * 0.90);
    float ridge2 = caustic_ridge(-q.x * 4.4 + q.y * 6.8 - u_time * 0.46
        + sin(q.x * 4.8 + u_time * 0.26) * 0.84);
    float ridge3 = caustic_ridge(q.x * 3.8 + q.y * 5.2 + u_time * 0.35
        + sin((q.x - q.y) * 3.7 - u_time * 0.19) * 0.62);
    float network = max(ridge1 * 0.88, max(ridge2 * 0.76, ridge3 * 0.60));
    float intersections = max(ridge1 * ridge2, max(ridge2 * ridge3, ridge1 * ridge3));

    float depth = 0.5 + 0.5 * sin(p.y * 1.8 + u_time * 0.17);
    float shimmer = 0.88 + 0.12 * sin(u_time * 1.7 + p.x * 3.1 - p.y * 1.8);
    float light = 0.055 + depth * 0.025;
    light += swells * 0.18;
    light += network * (0.20 + swells * 0.10) * shimmer;
    light += intersections * 0.14;
    light += u_selected * 0.07 * max(swells, network);
    light = clamp(light, 0.045, 0.72);

    vec3 school_blue = vec3(0.219608, 0.741176, 0.972549);
    vec3 deep_water = vec3(0.018, 0.105, 0.170);
    vec3 shadow = mix(vec3(0.035), deep_water, u_colored);
    vec3 highlight = mix(vec3(0.82), school_blue, u_colored);
    vec4 output_color = vec4(mix(shadow, highlight, light), v_color.a);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
