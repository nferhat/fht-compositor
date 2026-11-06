#version 100

precision highp float;

varying vec2 v_coords;

uniform sampler2D tex;
uniform vec2 half_pixel;
uniform vec2 uv_scale;
uniform float offset;

// Dual kawase upsample: 8 taps in a tent filter around the sample.
void main() {
    vec2 uv = v_coords * uv_scale;
    vec2 o = half_pixel * offset;

    vec4 sum = vec4(0.0);

    sum += texture2D(tex, uv + vec2(-o.x * 2.0, 0.0));
    sum += texture2D(tex, uv + vec2( o.x * 2.0, 0.0));
    sum += texture2D(tex, uv + vec2(0.0, -o.y * 2.0));
    sum += texture2D(tex, uv + vec2(0.0,  o.y * 2.0));

    sum += texture2D(tex, uv + vec2(-o.x,  o.y)) * 2.0;
    sum += texture2D(tex, uv + vec2( o.x,  o.y)) * 2.0;
    sum += texture2D(tex, uv + vec2(-o.x, -o.y)) * 2.0;
    sum += texture2D(tex, uv + vec2( o.x, -o.y)) * 2.0;

    gl_FragColor = sum / 12.0;
}
