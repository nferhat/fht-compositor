// Shader code for rounded corners support. Import using #include from other files.
// Logic from niri, thank you very much!

float rounding_alpha(vec2 coords, vec2 size, float radius) {
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

    float dist = distance(coords, center);
    return 1.0 - smoothstep(-0.5, +0.5, (dist - radius));
}
