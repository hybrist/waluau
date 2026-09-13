precision highp float;
varying vec2 v_uv;
uniform sampler2D u_texture;
uniform float u_clock;
uniform float u_strength;

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
    // Every wave is a scaled copy of this breathing outline. No angular
    // phase offset between radii: the light reads as circles, not spiral arms.
    float outline = 1.0 + 0.012 * sin(angle * 7.0 - t * 0.45)
        + 0.008 * sin(angle * 13.0 + t * 0.32);
    float radial = r / outline;
    // Keep the focus still; a small radial ripple refracts the blurred edges.
    vec2 warped = p * (1.0 + u_strength * edge * 0.018 * sin(radial * 24.0 - t * 1.4));
    vec2 uv = sampleUV(warped);
    vec2 spread = vec2(1.0 / 960.0, 1.0 / 640.0) * u_strength * edge * edge * 20.0;
    // Blur the sampled map itself, including the city outside the clear lens.
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

    vec3 violet = vec3(0.30, 0.13, 0.58);
    vec3 teal = vec3(0.34, 0.95, 0.79);
    float breath = 0.78 + 0.22 * sin(t * 1.4);
    float lip = 0.865 + 0.004 * sin(t * 1.4);
    float aperture = 1.0 - smoothstep(0.77, 1.02, radial);
    // A translucent dark veil leaves the surrounding blurred streets visible.
    // The final composite is opaque because it already contains that map.
    vec3 outside = city * vec3(0.32, 0.34, 0.43) + vec3(0.012, 0.008, 0.025);
    vec3 color = mix(outside, city, aperture);
    float ring = exp(-abs(radial - lip) * 150.0);
    float halo = exp(-abs(radial - lip) * 28.0);
    color += u_strength * breath * (teal * ring * 0.8 + violet * halo * 0.35);
    // Soft pulses travel out from the lip and fade before wrapping. Each
    // concentric ring inherits precisely the same lobes as the inner circle.
    for (int i = 0; i < 3; i++) {
        float phase = fract(t * 0.14 + float(i) / 3.0);
        float radius = lip + phase * 0.60;
        float envelope = pow(sin(phase * 3.14159265), 2.0);
        float wave = exp(-abs(radial - radius) * 85.0);
        float glow = exp(-abs(radial - radius) * 25.0);
        color += u_strength * envelope * (mix(teal, violet, phase) * wave * 0.28 + violet * glow * 0.10);
    }
    gl_FragColor = vec4(color, 1.0);
}
