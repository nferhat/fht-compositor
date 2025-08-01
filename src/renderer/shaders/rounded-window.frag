//! Texture shader to apply rounded corners onto a window.
//!
//! ## Uniforms
//! - geo_size: The size of the window
//! - corner_radius: The corner radius to apply
//! - input_to_geo: A 3x3 matrix transforming v_coords into coordinates local to the window rectangle

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#include "rounded-corners.glsl"

#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 geo_size;
uniform float corner_radius;
uniform mat3 input_to_geo;

void main() {
    vec3 coords_geo = input_to_geo * vec3(v_coords, 1.0);

    // Sample the texture.
    vec4 color = texture2D(tex, v_coords);
    #if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
    #endif

    if (coords_geo.x < 0.0 || 1.0 < coords_geo.x || coords_geo.y < 0.0 || 1.0 < coords_geo.y) {
        // The surface we are trying to draw is stricly outside the rectangle given.
        // Clip it out.
        color = vec4(0.0);
    } else {
        // Apply corner rounding inside geometry.
        color *= rounding_alpha(coords_geo.xy * geo_size, geo_size, corner_radius);
    }

    // Apply final alpha and tint.
    color = color * alpha;

    #if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
    #endif

    gl_FragColor = color;
}
