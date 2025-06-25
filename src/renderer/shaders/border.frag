//! Pixel shader to draw a border. The given border will be drawn inside the
//! shader area, so it grows inwards.
//!
//! ## Uniforms
//! - v_start/end_color and v_gradient_angle: The parameters for the color.
//! - corner_radius: The corner radius to apply.
//! - thickness: The border thickness to apply.

precision mediump float;
#include "rounded-corners.glsl"

uniform vec4 v_start_color;
uniform vec4 v_end_color;
uniform float v_gradient_angle;
uniform float corner_radius;
uniform float thickness;

uniform vec2 size;
uniform float alpha;
varying vec2 v_coords;

// Gradient color calculation from here
// https://www.shadertoy.com/view/Mt2XDK
vec4 get_pixel_color() {
    vec2 origin = vec2(0.5);
    vec2 uv = v_coords - origin;
    float angle = radians(90.0) - radians(v_gradient_angle) + atan(uv.x, uv.y);
    float uv_len = length(uv);
    uv = vec2(cos(angle) * uv_len, sin(angle) * uv_len) + origin;
    return mix(v_start_color, v_end_color, smoothstep(0.0, 1.0, uv.x));
}

void main() {
    vec2 loc = v_coords * size;
    vec4 color = get_pixel_color();
    color *= rounding_alpha(loc, size, corner_radius);

    if (thickness > 0.0) {
        // Second pass: inner rounding
        // We offset everything to be in the inner rectangle
        loc -= vec2(thickness);
        vec2 inner_size = size - vec2(thickness * 2.0);
        if (0.0 <= loc.x && loc.x <= inner_size.x && 0.0 <= loc.y && loc.y <= inner_size.y) {
            float inner_radius = max(corner_radius - thickness, 0.0);
            color = color * (1.0 - rounding_alpha(loc, inner_size, inner_radius));
        }
    }

    gl_FragColor = color * alpha;
}

// vim: ft=glsl
