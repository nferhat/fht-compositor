precision mediump float;

#define KIND_SOLID 0
#define KIND_GRADIENT 1
// Transform functions for gradient processing
// https://www.shadertoy.com/view/lscGDr
#define SRGB_TO_LINEAR(c) pow((c), vec4(2.2))
#define LINEAR_TO_SRGB(c) pow((c), vec4(1.0 / 2.2))

#include "rounded-corners.glsl"

uniform int color_kind;
uniform vec4 color_start;
uniform vec4 color_end;
uniform float color_angle;
uniform float corner_radius;
uniform float thickness;

uniform vec2 size;
uniform float alpha;
varying vec2 v_coords;

// Calculates the given gradient color with the passed-in v_coords
// It mixes in linear colorspace for more accurate color mixing.
vec4 gradient_color(vec4 start, vec4 end, float angle) {
    float rad = radians(angle);
    vec2 dir = vec2(cos(rad), sin(rad));
    vec2 centered = v_coords * 2.0 - 1.0;
    float t = dot(centered, dir);
    t = smoothstep(-1.0 / sqrt(2.0), 1.0 / sqrt(2.0), t * sqrt(2.0));
    // Transform the colors to srgb for mixing then bacck to linear
    vec4 color = mix(SRGB_TO_LINEAR(start), SRGB_TO_LINEAR(end), t);
    return LINEAR_TO_SRGB(color);
}

void main() {
    vec2 loc = v_coords * size;
    vec4 color;

    // First calculate the color
    if (color_kind == KIND_SOLID) {
        color = color_start;
    } else if (color_kind == KIND_GRADIENT) {
        color = gradient_color(color_start, color_end, color_angle);
    } else {
        discard; // invalid color? this should never happen
    }

    // First rounding pass is for outside radius.
    color *= rounding_alpha(loc, size, corner_radius);

    if (thickness > 0.0) {
        // Second pass: inner rounding
        loc -= vec2(thickness);
        vec2 inner_size = size - vec2(thickness * 2.0);

        // Only apply rounding when we are inside
        if (0.0 <= loc.x && loc.x <= inner_size.x && 0.0 <= loc.y && loc.y <= inner_size.y) {
            float inner_radius = max(corner_radius - thickness, 0.0);
            color = color * (1.0 - rounding_alpha(loc, inner_size, inner_radius));
        }
    }

    gl_FragColor = color * alpha;
}

// vim: ft=glsl
