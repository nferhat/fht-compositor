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

#include "rounded-corners.glsl"

void main()
{
    vec2 tex_coords = (v_coords * win_size) / curr_size;
    if (win_size.x > curr_size.x)
        tex_coords.x = v_coords.x;
    if (win_size.y > curr_size.y)
        tex_coords.y = v_coords.y;
    vec4 color = texture2D(tex, tex_coords);

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
