precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_aspect;
uniform float u_selected;
uniform float u_colored;

float branch(vec2 p, vec2 a, vec2 b, float width) {
    vec2 stem = b - a;
    float along = clamp(dot(p - a, stem) / dot(stem, stem), 0.0, 1.0);
    float distance_to_stem = length(p - (a + stem * along));
    float taper = width * (1.0 - along * 0.48);
    return 1.0 - smoothstep(taper, taper + 0.014, distance_to_stem);
}

vec2 leaf_fields(vec2 p, vec2 center, vec2 direction, float length, float width, float phase) {
    vec2 axis = normalize(direction);
    vec2 side = vec2(-axis.y, axis.x);
    float breathing = 0.86 + 0.14 * (0.5 + 0.5 * sin(u_time * 1.15 + phase));
    float longitudinal = dot(p - center, axis) / (length * breathing);
    float lateral = dot(p - center, side);
    float profile = width * breathing * pow(max(0.0, 1.0 - longitudinal * longitudinal), 0.72);
    float body = (1.0 - smoothstep(profile, profile + 0.014, abs(lateral)))
        * (1.0 - smoothstep(0.91, 1.0, abs(longitudinal)));
    float midrib = body * (1.0 - smoothstep(0.008, 0.020, abs(lateral)));
    return vec2(body, midrib);
}

void main() {
    vec2 p = vec2((v_uv.x - 0.5) * 2.0 * u_aspect, 1.0 - v_uv.y);

    // A shared wind displacement preserves every attachment while the crown
    // sways farther than the rooted base.
    float wind = (
        sin(u_time * 0.85 + p.y * 4.2) * 0.030
        + sin(u_time * 1.43 - p.y * 7.0) * 0.012
    ) * p.y * p.y;
    p.x -= wind;

    float trunk_curve = sin(p.y * 4.6 + u_time * 0.34) * 0.035 * p.y;
    float trunk_width = mix(0.058, 0.020, clamp(p.y, 0.0, 1.0));
    float trunk = (1.0 - smoothstep(trunk_width, trunk_width + 0.015, abs(p.x - trunk_curve)))
        * (1.0 - smoothstep(0.91, 0.98, p.y));

    float stems = trunk;
    stems = max(stems, branch(p, vec2(0.00, 0.25), vec2(-0.34, 0.48), 0.043));
    stems = max(stems, branch(p, vec2(0.01, 0.39), vec2(0.39, 0.65), 0.040));
    stems = max(stems, branch(p, vec2(-0.01, 0.53), vec2(-0.32, 0.75), 0.034));
    stems = max(stems, branch(p, vec2(0.00, 0.66), vec2(0.25, 0.88), 0.030));
    stems = max(stems, branch(p, vec2(-0.17, 0.62), vec2(-0.48, 0.66), 0.022));
    stems = max(stems, branch(p, vec2(0.20, 0.52), vec2(0.48, 0.48), 0.022));

    vec2 leaf1 = leaf_fields(p, vec2(-0.42, 0.52), vec2(-0.82, 0.58), 0.17, 0.080, 0.2);
    vec2 leaf2 = leaf_fields(p, vec2(0.47, 0.69), vec2(0.78, 0.63), 0.18, 0.085, 1.3);
    vec2 leaf3 = leaf_fields(p, vec2(-0.38, 0.79), vec2(-0.76, 0.65), 0.17, 0.082, 2.5);
    vec2 leaf4 = leaf_fields(p, vec2(0.29, 0.91), vec2(0.58, 0.82), 0.16, 0.075, 3.7);
    vec2 leaf5 = leaf_fields(p, vec2(-0.54, 0.66), vec2(-0.98, 0.18), 0.14, 0.067, 4.6);
    vec2 leaf6 = leaf_fields(p, vec2(0.54, 0.48), vec2(0.98, -0.08), 0.14, 0.067, 5.4);
    vec2 leaf7 = leaf_fields(p, vec2(-0.14, 0.37), vec2(-0.72, 0.69), 0.13, 0.061, 6.2);
    vec2 leaf8 = leaf_fields(p, vec2(0.16, 0.56), vec2(0.70, 0.71), 0.13, 0.061, 0.9);

    float leaves = max(max(max(leaf1.x, leaf2.x), max(leaf3.x, leaf4.x)),
        max(max(leaf5.x, leaf6.x), max(leaf7.x, leaf8.x)));
    float veins = max(max(max(leaf1.y, leaf2.y), max(leaf3.y, leaf4.y)),
        max(max(leaf5.y, leaf6.y), max(leaf7.y, leaf8.y)));

    // Brightness climbs from roots to tips like sap moving through the plant.
    float sap = 0.5 + 0.5 * sin(p.y * 20.0 - u_time * 3.4);
    sap *= sap;
    float root_glow = exp(-p.x * p.x * 6.0) * exp(-p.y * 5.0);
    float leaf_breath = 0.90 + 0.10 * sin(u_time * 1.3 + p.x * 5.0 + p.y * 3.0);
    float brightness = 0.065 + root_glow * 0.20;
    brightness += stems * (0.22 + sap * 0.30);
    brightness += leaves * (0.26 + sap * 0.17) * leaf_breath;
    brightness += veins * 0.22;
    brightness += u_selected * 0.09 * max(stems, leaves);

    vec3 school_green = vec3(0.290196, 0.870588, 0.501961);
    vec3 tint = mix(vec3(0.82), school_green, u_colored);
    vec4 output_color = vec4(tint * brightness, v_color.a);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
