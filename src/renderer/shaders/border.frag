precision mediump float;

uniform vec4 v_start_color;
uniform vec4 v_end_color;
uniform float v_gradient_angle;
uniform float corner_radius;
uniform float thickness;

uniform vec2 size;
uniform float alpha;
varying vec2 v_coords;

float rounding_alpha(vec2 coords, vec2 size, float radius)
{
    vec2 center;

    if (coords.x < corner_radius && coords.y < corner_radius)
    {
        center = vec2(radius);
    }
    else if (size.x - corner_radius < coords.x && coords.y < corner_radius)
    {
        center = vec2(size.x - radius, radius);
    }
    else if (size.x - corner_radius < coords.x && size.y - corner_radius < coords.y)
    {
        center = size - vec2(radius);
    }
    else if (coords.x < corner_radius && size.y - corner_radius < coords.y)
    {
        center = vec2(radius, size.y - radius);
    }
    else
    {
        return 1.0;
    }

    float dist = distance(coords, center);
    return 1.0 - smoothstep(radius - 0.5, radius + 0.5, dist);
}

// Gradient color calculation from here
// https://www.shadertoy.com/view/Mt2XDK
vec4 get_pixel_color()
{
    vec2 origin = vec2(0.5);
    vec2 uv = v_coords - origin;

    float angle = radians(90.0) - radians(v_gradient_angle) + atan(uv.x, uv.y);

    float uv_len = length(uv);
    uv = vec2(cos(angle) * uv_len, sin(angle) * uv_len) + origin;

    return mix(v_start_color, v_end_color, smoothstep(0.0, 1.0, uv.x));
}

void main()
{
    vec2 loc = v_coords * size;
    // First rounding pass is for outside radius
    vec4 color = get_pixel_color();
    color *= rounding_alpha(loc, size, corner_radius);

    if (thickness > 0.0)
    {
        // Second pass: inner rounding
        loc -= vec2(thickness);
        vec2 inner_size = size - vec2(thickness * 2.0);

        // Only apply rounding when we are inside
        if (0.0 <= loc.x && loc.x <= inner_size.x && 0.0 <= loc.y && loc.y <= inner_size.y)
        {
            float inner_radius = max(corner_radius - thickness, 0.0);
            color = color * (1.0 - rounding_alpha(loc, inner_size, inner_radius));
        }
    }

    gl_FragColor = color * alpha;
}

// vim: ft=glsl
