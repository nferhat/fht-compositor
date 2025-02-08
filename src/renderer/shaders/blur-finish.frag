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

uniform vec4 geo;
uniform float corner_radius;
uniform float noise;

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

// Noise function copied from hyprland.
// I like the effect it gave, can be tweaked further
float hash(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 727.727); // wysi :wink: :wink:
    p3 += dot(p3, p3.xyz + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

void main() {

    // Sample the texture.
    vec4 color = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

    // This shader exists to make blur rounding correct.
    // 
    // Since we are scr-ing a texture that is the size of the output, the v_coords are always
    // relative to the output. This corresponds to gl_FragCoord.
    vec2 size = geo.zw;
    vec2 loc = gl_FragCoord.xy - geo.xy;

    // Add noise fx
    // This can be used to achieve a glass look
    float noiseHash   = hash(loc / size);
    float noiseAmount = (mod(noiseHash, 1.0) - 0.5);
    color.rgb += noiseAmount * noise;

    // Apply corner rounding inside geometry.
    color *= rounding_alpha(loc, size, corner_radius);


    // Apply final alpha and tint.
    color *= alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}

// vim: ft=glsl
