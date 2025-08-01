//! Texture shader to draw a shadow. The actual shadow will be drawn around a rectangle, shrunk by
//! the value of `blur_sigma` (see uniforms down below)
//!
//! Credits: <https://madebyevan.com/shaders/fast-rounded-rectangle-shadows/>
//!
//! ## Uniforms
//! - corner_radius: The corner radius to apply.
//! - blur_sigma: The shadow blur radii/sigma.
//! - shadow_color: The shadow color.

precision highp float;
#include "rounded-corners.glsl"

uniform vec4 shadow_color;
uniform float blur_sigma;
uniform float corner_radius;

uniform vec2 size;
uniform float alpha;
varying vec2 v_coords;

// A standard gaussian function, used for weighting samples
float gaussian(float x, float sigma) {
    const float pi = 3.141592653589793;
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * pi) * sigma);
}

// This approximates the error function, needed for the gaussian integral
vec2 erf(vec2 x) {
    vec2 s = sign(x), a = abs(x);
    x = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    x *= x;
    return s - s / (x * x);
}

// Return the blurred mask along the x dimension
float rounded_box_shadow_x(float x, float y, float sigma, float corner, vec2 halfSize) {
    float delta = min(halfSize.y - corner - abs(y), 0.0);
    float curved = halfSize.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    vec2 integral = 0.5 + 0.5 * erf((x + vec2(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

// Return the mask for the shadow of a box from lower to upper
float rounded_box_shadow(vec2 lower, vec2 upper, vec2 point, float sigma, float corner) {
    // Center everything to make the math easier
    vec2 center = (lower + upper) * 0.5;
    vec2 halfSize = (upper - lower) * 0.5;
    point -= center;

    // The signal is only non-zero in a limited range, so don't waste samples
    float low = point.y - halfSize.y;
    float high = point.y + halfSize.y;
    float start = clamp(-3.0 * sigma, low, high);
    float end = clamp(3.0 * sigma, low, high);

    // Accumulate samples (we can get away with surprisingly few samples)
    float step = (end - start) / 4.0;
    float y = start + step * 0.5;
    float value = 0.0;
    for (int i = 0; i < 4; i++)
    {
        value += rounded_box_shadow_x(point.x, point.y - y, sigma, corner, halfSize) * gaussian(y, sigma) * step;
        y += step;
    }

    return value;
}

void main() {
    // the shader's element size will always fit the blur sigma / 2
    vec2 rect_pos = vec2(blur_sigma);
    vec2 rect_size = size - vec2(2. * blur_sigma);
    vec2 pos = v_coords * size;
    float frag_alpha = shadow_color.a;
    frag_alpha *= rounded_box_shadow(rect_pos, rect_pos + rect_size, pos, blur_sigma / 2., corner_radius);

    // Cut out the inner side, for transparent windows
    pos -= vec2(blur_sigma);
    if (0.0 <= pos.x && pos.x <= rect_size.x && 0.0 <= pos.y && pos.y <= rect_size.y)
        frag_alpha *= 1.0 - rounding_alpha(pos, rect_size, corner_radius);

    gl_FragColor = vec4(shadow_color.xyz * frag_alpha, frag_alpha) * alpha;
}

// vim: ft=glsl
