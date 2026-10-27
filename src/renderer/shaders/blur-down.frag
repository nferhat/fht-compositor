#version 100

precision highp float;

varying vec2 v_coords;

uniform sampler2D tex;
uniform vec2 half_pixel;
uniform vec2 uv_scale;
uniform float offset;

// Dual kawase downsample: centre sample weighted 4x plus four diagonal neighbours.
void main() {
    vec2 uv = v_coords * uv_scale;
    vec2 o = half_pixel * offset;

    vec4 sum = texture2D(tex, uv) * 4.0;
    sum += texture2D(tex, uv + vec2(-o.x, -o.y));
    sum += texture2D(tex, uv + vec2( o.x, -o.y));
    sum += texture2D(tex, uv + vec2(-o.x,  o.y));
    sum += texture2D(tex, uv + vec2( o.x,  o.y));

    gl_FragColor = sum / 8.0;
}
