precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_aspect;
uniform float u_strength;

float line_band(float phase, float inner, float outer) {
    float distance_to_line = abs(fract(phase) - 0.5);
    return 1.0 - smoothstep(inner, outer, distance_to_line);
}

float hash21(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

void main() {
    vec2 p = vec2(v_uv.x * u_aspect, v_uv.y);
    vec2 centred = (v_uv - 0.5) * 2.0;
    centred.x *= u_aspect;
    float radius = length(centred);
    float angle = atan(centred.y, centred.x);

    // Fine opposing filaments move at different speeds. Their narrow,
    // well-defined intersections give the light a lively woven shimmer.
    float etch_a = line_band(p.x * 1.65 + p.y * 2.4 - u_time * 0.34, 0.014, 0.035);
    float etch_b = line_band(p.x * 1.05 - p.y * 3.1 + u_time * 0.23, 0.009, 0.026);
    float filaments = max(etch_a * 0.72, etch_b * 0.48);
    float crossing = etch_a * etch_b;

    // A travelling reflection has a bright knife-edge followed by a broader
    // gold face, like one beam sweeping across the completed formation.
    float sweep_phase = fract(v_uv.x * 0.88 + v_uv.y * 0.28 - u_time * 0.105);
    float sweep_distance = abs(sweep_phase - 0.5);
    float sweep_face = 1.0 - smoothstep(0.055, 0.105, sweep_distance);
    float sweep_edge = 1.0 - smoothstep(0.006, 0.019, sweep_distance);

    // Sparse four-point glints belong to the same coordinate field, so they
    // travel cleanly through the gaps instead of restarting on every card.
    vec2 glint_grid = vec2(p.x * 1.15 - u_time * 0.12, p.y * 4.0);
    vec2 glint_cell = fract(glint_grid) - 0.5;
    vec2 glint_id = floor(glint_grid);
    float glint_gate = step(0.76, hash21(glint_id));
    float glint_core = 1.0 - smoothstep(0.015, 0.050, length(glint_cell));
    float glint_h = (1.0 - smoothstep(0.018, 0.050, abs(glint_cell.y)))
        * (1.0 - smoothstep(0.05, 0.25, abs(glint_cell.x)));
    float glint_v = (1.0 - smoothstep(0.018, 0.050, abs(glint_cell.x)))
        * (1.0 - smoothstep(0.05, 0.22, abs(glint_cell.y)));
    float glint = glint_gate * max(glint_core, max(glint_h, glint_v));

    // The completed hand throws a crisp sunburst beyond its silhouette.
    // Alternating spoke widths keep the rays irregular and celebratory, while
    // two travelling rings make the whole formation pulse outward.
    float spoke_phase = angle * 15.0
        + sin(angle * 5.0 - u_time * 0.42) * 0.58
        + u_time * 0.18;
    float narrow_spokes = 1.0 - smoothstep(0.935, 0.985, abs(sin(spoke_phase)));
    float fine_spokes = 1.0 - smoothstep(
        0.965,
        0.994,
        abs(sin(spoke_phase * 1.73 + 1.4))
    );
    float radial_fade = 1.0 - smoothstep(1.45, u_aspect, radius);
    float burst = max(narrow_spokes * 0.72, fine_spokes * 0.50) * radial_fade;

    float ring_radius_a = 0.72 + fract(u_time * 0.19) * 1.55;
    float ring_radius_b = 0.72 + fract(u_time * 0.19 + 0.50) * 1.55;
    float ring_a = (1.0 - smoothstep(0.020, 0.058, abs(radius - ring_radius_a)))
        * (1.0 - fract(u_time * 0.19));
    float ring_b = (1.0 - smoothstep(0.020, 0.058, abs(radius - ring_radius_b)))
        * (1.0 - fract(u_time * 0.19 + 0.50));
    float rings = max(ring_a, ring_b);

    // The drawing primitive is rectangular, but the light is a long capsule
    // around the hand. Rounded ends and a generous vertical falloff erase the
    // quad's footprint while keeping one coordinate field through every gap.
    float capsule_x = max(abs(centred.x) - (u_aspect - 0.78), 0.0);
    float capsule_distance = length(vec2(capsule_x, centred.y));
    float envelope = 1.0 - smoothstep(0.43, 1.0, capsule_distance);

    vec3 warm_gold = vec3(1.0, 0.60, 0.055);
    vec3 bright_gold = vec3(1.0, 0.82, 0.25);
    vec3 pale_gold = vec3(1.0, 0.97, 0.72);
    float brilliance = clamp(
        filaments * 0.45 + sweep_face * 0.30 + sweep_edge * 0.72
            + crossing * 0.38 + glint + burst * 0.38 + rings * 0.70,
        0.0,
        1.0
    );
    vec3 color = mix(warm_gold, bright_gold, 0.32 + brilliance * 0.48);
    color = mix(color, pale_gold, sweep_edge * 0.56 + glint * 0.82);

    // Only the moving light has substantial opacity. The faint base ties the
    // gaps together without laying an amber slab beneath the cards.
    float light = 0.012 + filaments * 0.28 + sweep_face * 0.15
        + sweep_edge * 0.52 + crossing * 0.22 + glint * 0.78
        + burst * 0.42 + rings * 0.68;
    float alpha = v_color.a * u_strength * envelope * min(light, 0.95);
    vec4 output_color = vec4(color, alpha);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
