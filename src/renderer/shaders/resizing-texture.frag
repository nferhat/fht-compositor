#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;

// the size of the window texture
uniform vec2 win_size;
// the size we should display with
uniform vec2 curr_size;
// sample coords inside curr_size
varying vec2 v_coords;
// The corner radius of the tile.
uniform float corner_radius;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float rounding_alpha(vec2 coords, vec2 size, float radius) {
    vec2 center;

    if (coords.x < corner_radius && coords.y < corner_radius) {
        center = vec2(radius);
    } else if (size.x - corner_radius < coords.x && coords.y < corner_radius) {
        center = vec2(size.x - radius, radius);
    } else if (size.x - corner_radius < coords.x && size.y - corner_radius < coords.y) {
        center = size - vec2(radius);
    } else if (coords.x < corner_radius && size.y - corner_radius < coords.y) {
        center = vec2(radius, size.y - radius);
    } else {
        return 1.0;
    }

    float dist = distance(coords, center);
    return 1.0 - smoothstep(radius - 0.5, radius + 0.5, dist);
}

void main() {
    vec4 color;

    vec2 tex_coords = (v_coords * win_size) / curr_size;
    if (win_size.x > curr_size.x)
        tex_coords.x = v_coords.x;
    if (win_size.y > curr_size.y)
        tex_coords.y = v_coords.y;

    color = texture2D(tex, tex_coords);
    if (corner_radius > 0.0)
        color *= rounding_alpha(v_coords * curr_size, curr_size, corner_radius);
    
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}

// vim: ft=glsl
