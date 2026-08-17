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

float expanding_ripple(vec2 p, vec2 origin, float phase) {
    float cycle = fract(u_time * 0.115 + phase);
    float radius = cycle * 1.22;
    float ring_distance = abs(length(p - origin) - radius);
    float ring = 1.0 - smoothstep(0.020, 0.070, ring_distance);
    return ring * sin(cycle * 3.141593) * (1.0 - cycle);
}

void main() {
    if (rounded_card_distance() > 0.0) discard;
    vec2 p = vec2((v_uv.x - 0.5) * 2.0 * u_aspect, (v_uv.y - 0.5) * 2.0);

    // Two slow refraction currents distort the coordinate field before any
    // light ridges are measured, producing curved cells instead of stripes.
    vec2 q = p;
    q.x += sin(p.y * 3.1 + u_time * 0.47) * 0.105;
    q.x += sin(p.y * 6.7 - u_time * 0.31) * 0.040;
    q.y += sin(p.x * 2.8 - u_time * 0.39) * 0.090;
    q.y += sin(p.x * 5.9 + u_time * 0.28) * 0.035;

    float ridge1 = caustic_ridge(q.x * 7.2 + q.y * 2.8 + u_time * 0.82
        + sin(q.y * 5.0 - u_time * 0.43) * 1.15);
    float ridge2 = caustic_ridge(-q.x * 4.9 + q.y * 8.1 - u_time * 0.66
        + sin(q.x * 5.7 + u_time * 0.37) * 1.05);
    float ridge3 = caustic_ridge(q.x * 5.4 + q.y * 6.3 + u_time * 0.51
        + sin((q.x - q.y) * 4.2 - u_time * 0.29) * 0.82);
    float network = max(ridge1, max(ridge2, ridge3));
    float intersections = max(ridge1 * ridge2, max(ridge2 * ridge3, ridge1 * ridge3));

    float ripple1 = expanding_ripple(p, vec2(-0.38, 0.18), 0.08);
    float ripple2 = expanding_ripple(p, vec2(0.46, -0.32), 0.59);
    float depth = 0.5 + 0.5 * sin(p.y * 2.2 + u_time * 0.24);
    float shimmer = 0.90 + 0.10 * sin(u_time * 2.0 + p.x * 3.4 - p.y * 2.1);
    float brightness = 0.065 + depth * 0.035;
    brightness += network * 0.34 * shimmer;
    brightness += intersections * 0.22;
    brightness += (ripple1 + ripple2) * 0.15;
    brightness += u_selected * 0.08 * network;
    brightness = clamp(brightness, 0.055, 0.82);

    vec3 school_blue = vec3(0.219608, 0.741176, 0.972549);
    vec3 tint = mix(vec3(0.82), school_blue, u_colored);
    vec4 output_color = vec4(tint * brightness, v_color.a);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
