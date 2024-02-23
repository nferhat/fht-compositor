precision mediump float;
uniform float alpha;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform vec2 size;
varying vec2 v_coords;
uniform float radius;

float rounded_box(vec2 center, vec2 size, float radius) {
    return length(max(abs(center) - size + radius, 0.0)) - radius;
}

void main() {
    vec2 center = size / 2.0;
    vec2 location = v_coords * size;
    vec4 mix_color;

    float distance = rounded_box(location - center, size / 2.0, radius);
    if (distance > 1.0) {
        gl_FragColor = vec4(0);
    } else {
        gl_FragColor = texture2D(tex, v_coords);
    }

}

// vim: ft=glsl
