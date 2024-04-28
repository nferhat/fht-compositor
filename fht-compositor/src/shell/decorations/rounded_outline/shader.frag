precision mediump float;

uniform vec4 v_start_color;
uniform vec4 v_end_color;
uniform vec2 v_gradient_direction;
uniform float radius;
uniform float half_thickness;

uniform vec2 size;
varying vec2 v_coords;

float roundedBoxSDF(vec2 center, vec2 size, float radius) {
    vec2 q = abs(center) - size + radius;
    return min(max(q.x,q.y),0.0) + length(max(q,0.0)) - radius;
}

void main() {
    vec2 half_size = size / 2.0;
    vec2 center = (v_coords * size) - half_size;

    float distance = roundedBoxSDF(center, half_size - vec2(half_thickness), radius - half_thickness);
    float smoothedAlphaOuter = 1.0 - smoothstep(-0.5, .5, distance - half_thickness);
    // Create an inner circle that isn't as anti-aliased as the outer ring
    float smoothedAlphaInner = 1.0 - smoothstep(-0.5, 0.25, distance + half_thickness);

    float dotProduct = dot(v_coords, v_gradient_direction);
    vec4 v_color = mix(v_start_color, v_end_color, smoothstep(0.0, 1.0, dotProduct));

    gl_FragColor = mix(vec4(0), v_color, smoothedAlphaOuter - smoothedAlphaInner);
}

// vim: ft=glsl
