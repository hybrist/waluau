precision mediump float;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_textured;
uniform float u_time;
uniform sampler2D u_texture;
uniform float u_aspect;
uniform float u_strength;

float hash21(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

void main() {
    vec2 p = vec2(v_uv.x * u_aspect, v_uv.y);
    vec2 centred = (v_uv - 0.5) * 2.0;
    centred.x *= u_aspect;
    float radius = length(centred);
    float angle = atan(centred.y, centred.x);

    // Two broken seals continually contract toward the formation. Their
    // notches and uneven intensity keep them oppressive instead of polished.
    float seal_phase_a = fract(radius * 0.48 + u_time * 0.16);
    float seal_phase_b = fract(radius * 0.48 + u_time * 0.16 + 0.5);
    float seal_a = 1.0 - smoothstep(0.020, 0.065, abs(seal_phase_a - 0.5));
    float seal_b = 1.0 - smoothstep(0.020, 0.065, abs(seal_phase_b - 0.5));
    float seal_notches = 0.38 + 0.62 * smoothstep(
        0.18,
        0.72,
        abs(sin(angle * 9.0 + sin(angle * 3.0) * 1.4))
    );
    float seals = max(seal_a, seal_b) * seal_notches;

    // Ash sinks in staggered columns. A brief ember head above some trails
    // gives the field motion without turning it into victorious sparkle.
    vec2 ash_grid = vec2(p.x * 2.3, v_uv.y * 7.0 + u_time * 0.72);
    vec2 ash_cell = fract(ash_grid) - 0.5;
    vec2 ash_id = floor(ash_grid);
    float ash_gate = step(0.48, hash21(ash_id));
    float ash_trail = (1.0 - smoothstep(0.025, 0.090, abs(ash_cell.x)))
        * (1.0 - smoothstep(0.04, 0.48, abs(ash_cell.y + 0.18)));
    float ash_head = 1.0 - smoothstep(
        0.025,
        0.075,
        length(vec2(ash_cell.x, ash_cell.y + 0.38))
    );
    float ash = ash_gate * max(ash_trail * 0.42, ash_head);

    // Crooked inward claws grow from the long edges of the field. They darken
    // the gaps between cards and visually close around the captured hand.
    float edge_distance = u_aspect - abs(centred.x);
    float claw_wave = sin(centred.y * 13.0 + u_time * 1.15)
        + sin(centred.y * 29.0 - u_time * 0.63) * 0.34;
    float claw_reach = 0.55 + 0.22 * claw_wave;
    float claws = 1.0 - smoothstep(claw_reach, claw_reach + 0.28, edge_distance);
    claws *= 0.60 + 0.40 * (1.0 - smoothstep(0.38, 0.96, abs(centred.y)));

    // As with the gold effect, a rounded envelope hides the backing quad.
    float capsule_x = max(abs(centred.x) - (u_aspect - 0.78), 0.0);
    float capsule_distance = length(vec2(capsule_x, centred.y));
    float envelope = 1.0 - smoothstep(0.43, 1.0, capsule_distance);
    float central_gloom = 1.0 - smoothstep(0.24, 1.28, radius);
    float pulse = 0.88 + 0.12 * sin(u_time * 2.1);

    vec3 dried_blood = vec3(0.22, 0.008, 0.025);
    vec3 ward_red = vec3(0.72, 0.035, 0.075);
    vec3 ash_red = vec3(0.96, 0.22, 0.22);
    float red_light = seals * 0.56 + ash * 0.72;
    vec3 color = mix(dried_blood, ward_red, min(red_light, 1.0));
    color = mix(color, ash_red, ash_head * ash_gate * 0.62);
    color *= 0.76 + pulse * 0.24;
    color *= 1.0 - claws * 0.58;

    float light = 0.10 + central_gloom * 0.12 + seals * 0.54
        + ash * 0.52 + claws * 0.32;
    float alpha = v_color.a * u_strength * envelope * min(light, 0.88);
    vec4 output_color = vec4(color, alpha);
    if (v_textured > 0.5) output_color *= texture2D(u_texture, v_uv);
    gl_FragColor = output_color;
}
