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

float tongue(vec2 p, float center, float width, float height, float phase, float speed) {
    float rise = clamp(p.y / height, 0.0, 1.0);
    float sway = sin(p.y * 9.0 + phase - u_time * speed) * width * (0.10 + rise * 0.24);
    sway += sin(p.y * 21.0 - phase * 1.7 + u_time * speed * 1.6) * width * rise * 0.07;
    float taper = width * (1.0 - pow(rise, 0.72));
    float side = 1.0 - smoothstep(taper * 0.54, taper, abs(p.x - center - sway));
    float dancing_tip = height + sin(u_time * speed * 1.35 + phase) * 0.055
        + sin(u_time * speed * 2.7 - phase) * 0.022;
    float cap = 1.0 - smoothstep(dancing_tip - 0.11, dancing_tip, p.y);
    return side * cap * smoothstep(0.0, 0.045, p.y);
}

void main() {
    if (rounded_card_distance() > 0.0) discard;
    // UV y runs top-to-bottom, so this makes y=0 the flame's bottom edge.
    vec2 p = vec2((v_uv.x - 0.5) * 2.0 * u_aspect, 1.0 - v_uv.y);

    float core = tongue(p, 0.0, 0.42, 0.96, 0.4, 3.0);
    float left = tongue(p, -0.28, 0.30, 0.69, 2.2, 3.7);
    float right = tongue(p, 0.30, 0.27, 0.76, 4.5, 3.3);
    float inner = tongue(p, 0.04, 0.20, 0.57, 5.7, 4.4);
    float flame = max(core, max(left, right));

    // Rising bands make brightness race up through the silhouette while a
    // tighter inner tongue flashes independently near the origin.
    float flow = 0.5 + 0.5 * sin(
        p.y * 25.0 - u_time * 8.5
        + sin(p.x * 8.0 + u_time * 2.1) * 1.25
    );
    flow *= flow;
    float base_glow = exp(-p.x * p.x * 4.6) * exp(-p.y * 4.0);
    float flicker = 0.90
        + 0.07 * sin(u_time * 13.0)
        + 0.03 * sin(u_time * 23.0 + 1.7);
    float brightness = 0.075 + base_glow * 0.24;
    brightness += flame * (0.22 + flow * 0.43) * flicker;
    brightness += inner * (0.14 + 0.10 * sin(u_time * 17.0 + p.y * 12.0));
    brightness += u_selected * 0.10 * flame;

    vec3 school_red = vec3(0.937255, 0.266667, 0.266667);
    vec3 tint = mix(vec3(0.82), school_red, u_colored);
    vec4 output_color = vec4(tint * brightness, v_color.a);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
