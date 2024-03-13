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

uniform vec2 size;
uniform float radius;

float rounded_box(vec2 center, vec2 size, float radius) {
    return length(max(abs(center) - size + radius, 0.0)) - radius;
}

void main() {
    vec2 center = size / 2.0;
    vec2 location = v_coords * size;

    float distance = rounded_box(location - center, size / 2.0, radius);
    vec4 mix_color;
    if (distance > 1.0) {
        mix_color = vec4(0);
    } else {
        mix_color = texture2D(tex, v_coords);
    }

#if defined(NO_ALPHA)
    mix_color = vec4(mix_color.rgb, 1.0) * alpha;
#else
    mix_color = mix_color * alpha;
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        mix_color = vec4(0.0, 0.3, 0.0, 0.2) + mix_color * 0.8;
#endif

    gl_FragColor = mix_color;
}

// vim: ft=glsl
