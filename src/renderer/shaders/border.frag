precision mediump float;

uniform vec4 v_start_color;
uniform vec4 v_end_color;
uniform float v_gradient_angle;
uniform float radius;
uniform float half_thickness;

uniform vec2 size;
uniform float alpha;
varying vec2 v_coords;

float roundedBoxSDF(vec2 center, vec2 size, float radius) {
    vec2 q = abs(center) - size + radius;
    return min(max(q.x,q.y),0.0) + length(max(q,0.0)) - radius;
}

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
    vec2 half_size = size / 2.0;
    vec2 coords = v_coords * size;
    vec2 center = coords - half_size;

    float distance = roundedBoxSDF(center, half_size - vec2(half_thickness), radius - half_thickness);
    float smoothedAlphaOuter = 1.0 - smoothstep(-0.5, .5, distance - half_thickness);
    // Create an inner circle that isn't as anti-aliased as the outer ring
    float smoothedAlphaInner = 1.0 - smoothstep(-0.5, 0.25, distance + half_thickness);

    vec4 v_color = get_pixel_color();
    v_color.a = alpha;
    gl_FragColor = mix(vec4(0), v_color, smoothedAlphaOuter - smoothedAlphaInner);
}

// vim: ft=glsl
