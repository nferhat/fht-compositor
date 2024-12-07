precision mediump float;

uniform vec4 shadow_color;
uniform float blur_sigma;
uniform float corner_radius;

uniform vec2 size;
uniform float alpha;
varying vec2 v_coords;

// The shader code is from here
// https://madebyevan.com/shaders/fast-rounded-rectangle-shadows/

// A standard gaussian function, used for weighting samples
float gaussian(float x, float sigma)
{
    const float pi = 3.141592653589793;
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * pi) * sigma);
}

// This approximates the error function, needed for the gaussian integral
vec2 erf(vec2 x)
{
    vec2 s = sign(x), a = abs(x);
    x = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    x *= x;
    return s - s / (x * x);
}

// Return the blurred mask along the x dimension
float rounded_box_shadow_x(float x, float y, float sigma, float corner, vec2 halfSize)
{
    float delta = min(halfSize.y - corner - abs(y), 0.0);
    float curved = halfSize.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    vec2 integral = 0.5 + 0.5 * erf((x + vec2(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

// Return the mask for the shadow of a box from lower to upper
float rounded_box_shadow(vec2 lower, vec2 upper, vec2 point, float sigma, float corner)
{
    // Center everything to make the math easier
    vec2 center = (lower + upper) * 0.5;
    vec2 halfSize = (upper - lower) * 0.5;
    point -= center;

    // The signal is only non-zero in a limited range, so don't waste samples
    float low = point.y - halfSize.y;
    float high = point.y + halfSize.y;
    float start = clamp(-3.0 * sigma, low, high);
    float end = clamp(3.0 * sigma, low, high);

    // Accumulate samples (we can get away with surprisingly few samples)
    float step = (end - start) / 4.0;
    float y = start + step * 0.5;
    float value = 0.0;
    for (int i = 0; i < 4; i++)
    {
        value += rounded_box_shadow_x(point.x, point.y - y, sigma, corner, halfSize) * gaussian(y, sigma) * step;
        y += step;
    }

    return value;
}

// per-pixel "random" number between 0 and 1
float random()
{
    return fract(sin(dot(vec2(12.9898, 78.233), gl_FragCoord.xy)) * 43758.5453);
}

// simple rounded box sdf to check that we are inside
// https://iquilezles.org/articles/distfunctions2d/
float rounded_box_sdf(vec2 pos, vec4 rect, float corner_radius)
{
    vec2 half_size = (rect.zw) * 0.5;
    vec2 q = abs(pos - rect.xy - half_size) - half_size + corner_radius;
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - corner_radius;
}

void main()
{
    // the shader's element size will always fit the blur sigma / 2
    // FIXME: Avoid excess size??? This works fine but some pixels are unused.
    vec4 rect = vec4(vec2(blur_sigma), size.x - (2. * blur_sigma), size.y - (2. * blur_sigma));
    vec2 pos = v_coords * size;
    if (rounded_box_sdf(pos, rect, corner_radius) < 0.0)
        discard; // we dont draw the shadow *inside* the rectangle

    // First rounding pass is for outside radius
    float frag_alpha = shadow_color.a;
    frag_alpha *= rounded_box_shadow(rect.xy, rect.xy + rect.zw, v_coords * size, blur_sigma / 2., corner_radius);
    frag_alpha += (random() - 0.5) / 128.0;

    gl_FragColor = vec4(shadow_color.xyz * frag_alpha, frag_alpha) * alpha;
}

// vim: ft=glsl
