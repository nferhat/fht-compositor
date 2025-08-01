#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

varying vec2 v_coords;
uniform float radius;
uniform vec2 half_pixel;

void main() {
    vec2 uv = v_coords * 2.0;

    vec4 sum = texture2D(tex, uv) * 4.0;
    sum += texture2D(tex, uv - half_pixel.xy * radius);
    sum += texture2D(tex, uv + half_pixel.xy * radius);
    sum += texture2D(tex, uv + vec2(half_pixel.x, -half_pixel.y) * radius);
    sum += texture2D(tex, uv - vec2(half_pixel.x, -half_pixel.y) * radius);

    gl_FragColor = sum / 8.0;
}

