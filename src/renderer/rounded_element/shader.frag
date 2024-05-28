// Implementation from pinnacle-comp/pinnacle (GPL-3.0)
// Thank you very much!
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
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 geo_size;
uniform float corner_radius;
uniform mat3 input_to_geo;

float rounding_alpha(vec2 coords, vec2 size) {
    vec2 center;
    float radius;
    // HACK: To avoid a visual gap between the rounded surface and the border around it, subtract two
    // from the corner radius.
    //
    // Hacky? Yeah, can't do anything abt it.
    float corner_radius = corner_radius - 2.;

    if (coords.x < corner_radius && coords.y < corner_radius) {
        radius = corner_radius;
        center = vec2(radius, radius);
    } else if (size.x - corner_radius < coords.x && coords.y < corner_radius) {
        radius = corner_radius;
        center = vec2(size.x - radius, radius);
    } else if (size.x - corner_radius < coords.x && size.y - corner_radius < coords.y) {
        radius = corner_radius;
        center = vec2(size.x - radius, size.y - radius);
    } else if (coords.x < corner_radius && size.y - corner_radius < coords.y) {
        radius = corner_radius;
        center = vec2(radius, size.y - radius);
    } else {
        return 1.0;
    }

    float dist = distance(coords, center);
    return 1.0 - smoothstep(radius - 0.5, radius + 0.5, dist);
}

void main() {
    vec3 coords_geo = input_to_geo * vec3(v_coords, 1.0);

    // Sample the texture.
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

    if (coords_geo.x < 0.0 || 1.0 < coords_geo.x || coords_geo.y < 0.0 || 1.0 < coords_geo.y) {
        // Clip outside geometry.
        color = vec4(0.0);
    } else {
        // Apply corner rounding inside geometry.
        color = color * rounding_alpha(coords_geo.xy * geo_size, geo_size);
    }

    // Apply final alpha and tint.
    color = color * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}

// vim: ft=glsl
