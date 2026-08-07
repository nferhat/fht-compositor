// Shader code to create an SDF of a rounded rectangle, capable of adapting to create
// squircles and others. Based on superellipses.
//
// Original rounded rectangle code from https://github.com/niri-wm/niri
// Very nice people!
float rounding_alpha(vec2 coords, vec2 size, float radius, float power) {
    vec2 center;

    if (coords.x < radius && coords.y < radius) {
        center = vec2(radius);
    } else if (size.x - radius < coords.x && coords.y < radius) {
        center = vec2(size.x - radius, radius);
    } else if (size.x - radius < coords.x && size.y - radius < coords.y) {
        center = size - vec2(radius);
    } else if (coords.x < radius && size.y - radius < coords.y) {
        center = vec2(radius, size.y - radius);
    } else {
        return 1.0;
    }

    vec2 d = abs(coords - center);
    float dist = pow(pow(d.x, power) + pow(d.y, power), 1.0 / power);
    return 1.0 - smoothstep(-0.5, +0.5, (dist - radius));
}
