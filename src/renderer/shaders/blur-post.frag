//! Texture shader applied to the blurred backdrop of every blur element.
//!
//! It rounds the corners of the blurred rectangle like the window, optionally draws additional effects
//! ontop of the blurred texture. Right now we only have a liquid glass-esque material/effect from a
//! hyprland plugin.

precision highp float;
#include "rounded-corners.glsl"

uniform sampler2D tex;
uniform float alpha;
varying vec2 v_coords;

uniform vec2 geo_size;
uniform vec2 rect_origin;
uniform vec2 rect_size;
uniform float corner_radius;
uniform float corner_power;

// Options related to liquid-glass being enabled or not.
// I know always passing them isn't the best thing of all time but its fine.
uniform float glass_enabled;
uniform float edge_thickness;
uniform float refraction_strength;
uniform float lens_distortion;
uniform float chromatic_aberration;
uniform float fresnel_strength;
uniform float specular_strength;
uniform float glass_opacity;

uniform float brightness;
uniform float contrast;
uniform float saturation;
uniform float vibrancy;
uniform float dark_vibrancy;
uniform vec4 tint_color;

// Texture uv of a pixel inside the rectangle, clamped to it.
vec2 uv_at(vec2 pixel) {
    return rect_origin + clamp(pixel / geo_size, 0.0, 1.0) * rect_size;
}

// The backdrop refracted along the edges of the rectangle, with a fresnel rim and a specular edge.
// The liquid glass material is adapted from the glass shader of 0xdilo/hyprland-liquid-glass (MIT).
vec3 liquid_glass(vec2 pixel) {
    vec2 size = geo_size;
    float dim = max(min(size.x, size.y), 1.0);

    // Position relative to the center, and the signed distance to the rectangle edges.
    vec2 p = pixel - size * 0.5;
    vec2 h = size * 0.5;
    vec2 q = abs(p) - h;
    float sdf = min(max(q.x, q.y), 0.0) + length(max(q, 0.0));

    float inverse_sdf = max(-sdf / dim, 0.0);
    float distortion_depth = clamp(edge_thickness * 5.0, 0.02, 0.45);

    // normal of the glass
    float blend = max(distortion_depth * dim, 1.0);
    vec2 edge_dist = max(h - abs(p), 0.0);
    vec2 edge_w = 1.0 - smoothstep(vec2(0.0), vec2(blend), edge_dist);
    vec2 n = sign(p) * edge_w;
    n = n / max(length(n), 1e-4);

    // the best effects is done with a convex lens, IE distortion on the side of the window
    float t = clamp(inverse_sdf / distortion_depth, 0.0, 1.0);
    float d = 1.0 - t;
    float distortion = 1.0 - sqrt(max(1.0 - d * d, 0.0));

    float distortion_strength = clamp(lens_distortion * refraction_strength * 0.1135, 0.0, 0.45);
    vec2 offset = distortion * n * (size * 0.5) * distortion_strength;
    vec2 sample_pixel = pixel - offset;

    // chromatic aberration, since glass reflects blue more than it does red/green
    float chroma_shift = clamp(chromatic_aberration * 3.3333333, 0.0, 12.0);
    vec2 shift = n * distortion * chroma_shift;
    float r = texture2D(tex, uv_at(sample_pixel - shift)).r;
    float g = texture2D(tex, uv_at(sample_pixel)).g;
    float b = texture2D(tex, uv_at(sample_pixel + shift)).b;
    vec3 color = vec3(r, g, b);

    // fresnel rim, a specular highlight from a fixed light, and a little shade facing away.
    vec2 light = normalize(vec2(-0.65, -1.0));
    float facing_light = max(dot(n, light), 0.0);
    float facing_away = max(dot(n, -light), 0.0);
    float rim = pow(clamp(distortion, 0.0, 1.0), 1.35);
    float fresnel = rim * fresnel_strength * 0.035;
    float specular = pow(facing_light, 6.0) * rim * specular_strength * 0.10;
    float edge_shade = facing_away * rim * specular_strength * 0.025;
    return color * (1.0 - edge_shade) + vec3(fresnel + specular);
}

vec3 tone(vec3 color) {
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(luma), color, saturation);
    color = (color - 0.5) * contrast + 0.5;

    // Vibrancy boosts the saturation of the colors that are not saturated yet. Dark vibrancy only
    // does it in the shadows.
    float chroma = max(max(color.r, color.g), color.b) - min(min(color.r, color.g), color.b);
    float shadow = 1.0 - smoothstep(0.12, 0.55, dot(color, vec3(0.2126, 0.7152, 0.0722)));
    float vibrance = 1.0 + (vibrancy + dark_vibrancy * shadow) * (1.0 - clamp(chroma, 0.0, 1.0)) * 0.30;
    color = mix(vec3(dot(color, vec3(0.2126, 0.7152, 0.0722))), color, vibrance);

    color *= brightness;
    color = mix(color, tint_color.rgb, tint_color.a);
    return clamp(color, 0.0, 1.0);
}

void main() {
    vec2 pixel = (v_coords - rect_origin) / rect_size * geo_size;
    // Coverage of the rounded rectangle, outside of it the window below shows through.
    float corner = rounding_alpha(pixel, geo_size, corner_radius, corner_power);

    vec3 color;
    float material_alpha;
    if (glass_enabled > 0.5) {
        color = liquid_glass(pixel);
        material_alpha = clamp(glass_opacity / 0.78, 0.0, 1.0);
    } else {
        vec4 backdrop = texture2D(tex, v_coords);
        color = backdrop.rgb;
        material_alpha = backdrop.a;
    }

    // Output is premultiplied, like the blend the element is drawn with.
    gl_FragColor = vec4(tone(color) * material_alpha, material_alpha) * alpha * corner;
}
