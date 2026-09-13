precision highp float;
varying vec2 v_uv;
uniform sampler2D u_texture;
uniform float u_clock;
uniform float u_strength;

vec2 turn(vec2 p, float angle) {
    float c = cos(angle), s = sin(angle);
    return mat2(c, -s, s, c) * p;
}

// Coordinates are in units of the lens radius, preserving a circle on 960x640.
vec2 sampleUV(vec2 p) {
    return clamp(p * vec2(0.3333333, 0.5) + 0.5, vec2(0.002), vec2(0.998));
}

void main() {
    vec2 p = (v_uv - 0.5) * vec2(3.0, 2.0);
    float r = length(p);
    float angle = atan(p.y, p.x);
    float t = u_clock;
    float edge = smoothstep(0.22, 0.98, r);
    // The focal point never moves. Refraction and rotation gather at the rim.
    float twist = u_strength * edge * edge * (0.48 + 0.12 * sin(t * 0.65 - r * 7.0));
    vec2 warped = turn(p, twist);
    warped *= 1.0 + u_strength * edge * 0.025 * sin(r * 32.0 - t * 2.0);
    vec2 uv = sampleUV(warped);
    vec2 spread = vec2(1.0 / 960.0, 1.0 / 640.0) * u_strength * edge * edge * 13.0;
    // True image blur, including tangential smearing, rather than a fog overlay.
    vec3 city = texture2D(u_texture, uv).rgb * 0.2;
    for (int i = 0; i < 8; i++) {
        float a = float(i) * 0.78539816 + angle + t * 0.08;
        city += texture2D(u_texture, clamp(uv + vec2(cos(a), sin(a)) * spread, 0.002, 0.998)).rgb * 0.1;
    }
    vec2 split = normalize(p + vec2(0.0001)) * spread * 0.55;
    city.r = mix(city.r, texture2D(u_texture, clamp(uv + split, 0.002, 0.998)).r, edge * 0.55);
    city.b = mix(city.b, texture2D(u_texture, clamp(uv - split, 0.002, 0.998)).b, edge * 0.55);
    float luminance = dot(city, vec3(0.299, 0.587, 0.114));
    vec3 enchanted = mix(city * vec3(0.60, 0.84, 0.99), luminance * vec3(0.42, 1.03, 0.88), 0.28);
    city = mix(city, enchanted, min(u_strength, 1.0));

    float spiral = sin(angle * 5.0 - r * 23.0 + t * 1.3 + sin(angle * 3.0 + t * 0.4));
    float veil = pow(0.5 + 0.5 * spiral, 5.0) * edge;
    vec3 mist = mix(vec3(0.19, 0.045, 0.42), vec3(0.12, 0.83, 0.70), 0.5 + 0.5 * sin(angle * 2.0 - t * 0.5 + r * 9.0));
    vec3 voidColor = vec3(0.018, 0.012, 0.045);
    float aperture = 1.0 - smoothstep(0.77, 0.99, r);
    vec3 color = mix(voidColor, city, aperture);
    color += mist * veil * 0.42 * u_strength * (1.0 - smoothstep(1.0, 1.4, r));
    // Broken, breathing concentric filaments, with an irregular bright inner lip.
    float lip = 0.865 + 0.009 * sin(angle * 7.0 - t) + 0.006 * sin(angle * 13.0 + t * 0.7);
    float ring = exp(-abs(r - lip) * 150.0);
    float halo = exp(-abs(r - lip) * 24.0);
    float arc = pow(0.5 + 0.5 * sin(angle * 3.0 + t * 0.8), 3.0);
    float outer = exp(-abs(r - 0.935 - 0.006 * sin(angle * 9.0 + t)) * 230.0) * arc;
    color += u_strength * (vec3(0.34, 0.95, 0.79) * ring * (0.45 + arc) + mist * halo * 0.35 + vec3(0.65, 0.31, 0.98) * outer * 0.7);
    gl_FragColor = vec4(color, 1.0);
}
