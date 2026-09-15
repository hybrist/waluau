precision highp float;
varying vec2 v_uv;
uniform sampler2D u_texture;
uniform float u_clock;
uniform float u_overlay;
uniform float u_width;
uniform float u_height;
uniform float u_focus_x;
uniform float u_focus_y;
uniform float u_opacity;
uniform float u_strength;
uniform float u_static_circle;
uniform float u_peak_time;
uniform float u_travel_time;

// Integral of a smoothstep velocity ramp up, then down. Speed and
// acceleration are continuous at the peak; speed is zero at both endpoints.
// Normalization makes the final distance exactly one for any peak time.
float ringTravel(float age, float peak) {
    if (age < peak) {
        float x = age / peak;
        return peak * (2.0 * x * x * x - x * x * x * x);
    }
    float y = (age - peak) / (1.0 - peak);
    return peak + (1.0 - peak) * (2.0 * y - 2.0 * y * y * y + y * y * y * y);
}

// Coordinates are in lens radii, keeping the focus circular in any viewport.
vec2 sampleUV(vec2 p) {
    return clamp(p * (min(u_width, u_height) * 0.5) / vec2(u_width, u_height) + vec2(u_focus_x, u_focus_y), vec2(0.002), vec2(0.998));
}

void main() {
    vec2 p = (v_uv - vec2(u_focus_x, u_focus_y)) * vec2(u_width, u_height) / (min(u_width, u_height) * 0.5);
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
    // Outside the aperture, the city buckles like an image through moving
    // thick glass. Broad crossed waves bend streets and stretch rooftops;
    // their mask follows the wavy rim and leaves the central view untouched.
    float outerWarp = smoothstep(0.84, 1.18, radial) * u_strength;
    vec2 current = vec2(
        sin(p.y * 6.0 + t * 0.72 + sin(p.x * 3.5 - t * 0.31)),
        sin(p.x * 5.5 - t * 0.61 + sin(p.y * 4.0 + t * 0.27))
    );
    current += 0.3 * vec2(sin(p.y * 11.0 - t * 0.43), cos(p.x * 10.0 + t * 0.37));
    // Pull the outer image inward as it refracts, providing sampling room at
    // the screen edges instead of stretching clamped edge texels into bands.
    warped += outerWarp * (current * 0.085 - p * 0.14);
    vec2 uv = sampleUV(warped);
    // The route uses the identical refraction, but keeps its ink and sharpness.
    // Offscreen alpha blending stores premultiplied RGB; undo it for the
    // screen's straight-alpha blend, applying the handover opacity only once.
    if (u_overlay > 0.5) {
        vec4 route = texture2D(u_texture, uv);
        gl_FragColor = vec4(route.rgb / max(route.a, 0.00001), route.a * u_opacity);
        return;
    }
    vec2 spread = vec2(1.0 / u_width, 1.0 / u_height) * u_strength * edge * edge * 20.0;
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
    // Opacity hands the map over to the duel during the camera dive.
    vec3 outside = city * vec3(0.32, 0.34, 0.43) + vec3(0.012, 0.008, 0.025);
    vec3 color = mix(outside, city, aperture);
    float ring = exp(-abs(radial - lip) * 150.0);
    float halo = exp(-abs(radial - lip) * 28.0);
    color += u_static_circle * u_strength * breath * (teal * ring * 0.8 + violet * halo * 0.35);
    // Soft pulses travel out from the lip and fade before wrapping. Each
    // concentric ring inherits precisely the same lobes as the inner circle.
    float duration = max(u_travel_time, 0.1);
    float peak = clamp(u_peak_time / duration, 0.001, 0.999);
    for (int i = 0; i < 3; i++) {
        float phase = fract(t / duration + float(i) / 3.0);
        float radius = lip + ringTravel(phase, peak) * 0.60;
        // Reveal the acceleration, then fade during the final settling interval.
        float envelope = smoothstep(0.0, min(peak, 0.08), phase)
            * (1.0 - smoothstep(0.55, 1.0, phase));
        float wave = exp(-abs(radial - radius) * 85.0);
        float glow = exp(-abs(radial - radius) * 25.0);
        color += u_strength * envelope * (mix(teal, violet, phase) * wave * 0.28 + violet * glow * 0.10);
    }
    gl_FragColor = vec4(color, u_opacity);
}
