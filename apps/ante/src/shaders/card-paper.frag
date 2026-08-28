precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform sampler2D u_texture;
uniform float u_aspect;
// 0: opaque card stock. 1: the press pass — the same board, emitted as a
// sparse translucent layer over whatever was printed, so ink breaks up on
// the high tooth and wears thin toward the handled edges.
uniform float u_press;

float rounded_card_distance() {
    float radius = 0.0625;
    vec2 point = vec2(v_uv.x * u_aspect, v_uv.y);
    vec2 half_size = vec2(u_aspect * 0.5, 0.5);
    vec2 q = abs(point - half_size) - (half_size - radius);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123);
}

float value_noise(vec2 p) {
    vec2 cell = floor(p);
    vec2 part = fract(p);
    vec2 fade = part * part * (3.0 - 2.0 * part);
    float a = hash(cell);
    float b = hash(cell + vec2(1.0, 0.0));
    float c = hash(cell + vec2(0.0, 1.0));
    float d = hash(cell + vec2(1.0, 1.0));
    return mix(mix(a, b, fade.x), mix(c, d, fade.x), fade.y);
}

float pulp(vec2 p) {
    float total = value_noise(p) * 0.5;
    total += value_noise(p * 2.17 + vec2(31.7, 17.3)) * 0.25;
    total += value_noise(p * 4.31 + vec2(11.9, 47.1)) * 0.125;
    total += value_noise(p * 8.53 + vec2(73.3, 5.7)) * 0.0625;
    return total;
}

void main() {
    float distance_in = rounded_card_distance();
    if (distance_in > 0.0) discard;
    // Card-shaped units: y spans one card height, x scaled to match, so the
    // board texture keeps its scale whatever quad carries it.
    vec2 p = vec2(v_uv.x * u_aspect, v_uv.y);

    // Pressed board pulp: broad uneven mottling, long fibers laid along the
    // card's grain with a fainter cross-grain, and a fine printing tooth.
    float mottle = pulp(p * 7.0) - 0.47;
    float fiber = value_noise(vec2(p.x * 120.0, p.y * 8.0)) - 0.5;
    float cross_fiber = value_noise(vec2(p.x * 9.0, p.y * 140.0)) - 0.5;
    float tooth = hash(floor(p * 430.0)) - 0.5;

    vec3 board = vec3(0.150, 0.128, 0.148);
    board += mottle * vec3(0.075, 0.062, 0.052);
    board += fiber * 0.034 + cross_fiber * 0.020;
    board += tooth * 0.026;

    // Recycled stock carries occasional lighter chips of foreign pulp.
    float chip = smoothstep(0.965, 1.0, value_noise(p * 37.0 + vec2(5.1, 9.2)));
    board += chip * vec3(0.055, 0.048, 0.036);

    // Handling wear: the board darkens and roughens toward the cut, and the
    // raw cut itself catches light in a ragged sliver right at the rim.
    float rim = smoothstep(-0.085, 0.0, distance_in);
    board *= 1.0 - rim * (0.20 + 0.16 * value_noise(p * 55.0));
    float cut = smoothstep(-0.010, -0.003, distance_in);
    board += cut * (0.35 + 0.65 * value_noise(p * 95.0)) * vec3(0.085, 0.075, 0.060);

    // A gentle press vignette keeps the middle the brightest printable area.
    vec2 centered = p - vec2(u_aspect * 0.5, 0.5);
    board *= 1.0 - dot(centered, centered) * 0.30;

    // The printed frame: a worn double rule inset from the edge, following
    // the same rounded silhouette. Ink coverage breaks up over the texture,
    // as a letterpress line does on rough stock.
    float worn = 0.45 + 0.55 * value_noise(p * 27.0 + vec2(2.3, 6.1));
    float outer_rule = 1.0 - smoothstep(0.0022, 0.0044, abs(distance_in + 0.046));
    float inner_rule = 1.0 - smoothstep(0.0012, 0.0030, abs(distance_in + 0.060));
    vec3 rule_ink = vec3(0.760, 0.700, 0.570);
    board = mix(board, rule_ink, (outer_rule * 0.34 + inner_rule * 0.16) * worn);

    // The press pass shows the board through the ink instead of the board
    // itself: mostly nothing, with the highest grain poking through and the
    // worn rim letting more paper back in. Both passes derive from the same
    // fields, so a break in the ink always lands on a visible ridge.
    float ridge = (tooth + 0.5) * 0.55 + value_noise(p * 180.0 + vec2(8.7, 3.9)) * 0.45;
    float break_through = smoothstep(0.60, 0.97, ridge) * 0.42
        + max(fiber, 0.0) * 0.16
        + 0.05;
    break_through *= 1.0 + rim * 0.9;
    // The press pass renders once into a bake target and is composited from
    // there, so its alpha passes through blending twice; the fractional power
    // gives the twice-blended result roughly the authored coverage.
    float press_alpha = pow(min(break_through, 0.8), 0.75) * v_color.a;

    vec4 output_color = mix(
        vec4(board, v_color.a),
        vec4(board, press_alpha),
        u_press);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
