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

varying vec2 v_coords;
uniform vec2 half_pixel;
uniform float radius;

void main() {
    vec2 uv = v_coords / 2.0;

    vec4 sum = texture2D(tex, uv + vec2(-half_pixel.x * 2.0, 0.0) * radius);
    sum += texture2D(tex, uv + vec2(-half_pixel.x, half_pixel.y) * radius) * 2.0;
    sum += texture2D(tex, uv + vec2(0.0, half_pixel.y * 2.0) * radius);
    sum += texture2D(tex, uv + vec2(half_pixel.x, half_pixel.y) * radius) * 2.0;
    sum += texture2D(tex, uv + vec2(half_pixel.x * 2.0, 0.0) * radius);
    sum += texture2D(tex, uv + vec2(half_pixel.x, -half_pixel.y) * radius) * 2.0;
    sum += texture2D(tex, uv + vec2(0.0, -half_pixel.y * 2.0) * radius);
    sum += texture2D(tex, uv + vec2(-half_pixel.x, -half_pixel.y) * radius) * 2.0;

    gl_FragColor = sum / 12.0;
}

