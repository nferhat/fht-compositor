# Decorations

`fht-compositor` provides decorations: optional visual effects that can give off a nice looking effect. While they are not strictly required, they
still enhance the looks of the desktop session

> [!TIP] Window rules
> All the decorations values can be overridden for specific windows using [window rules](/configuration/window-rules)!

## Borders

Borders are an outline drawn around windows in a workspace. The focused window will get a different border color to indicate that it is focused and that
it has active keyboard focus. Borders can also apply rounded corner radius aroud windows.

> [!TIP] About colors
>
> Colors in the configuration are parsed using a CSS-style parser, so you can use either of
> 1. CSS named colors like `"red"`, `"blue"`, ...
> 2. Colors in hex notation (supports 3-character): `#rrggbb(aa)`
> 3. Color notation using functions like `rgba()`, `hsl()`, `rgb()`...

#### `border.focused-color`, `border.normal-color`


The border color for the focused and unfocused windows. The compositor optionally supports gradient borders, akin to CSS' `linear-gradient`, taking a
start color, end color, and an angle (in degrees).

::: tabs
== Simple solid border
```toml
[decorations.border]
focused-color = "#6791c9"
normal-color = "transparent"
```
== Gradient border
```toml
[decorations.border]
focused-color = { start = "#87c7a1", end = "#96d6b0", angle = 0 }
normal-color = "#101112"
```
:::

> [!TIP]
> When deserializing the configuration, we make use of tagged unions. Depending on the value that you assign to the color value, we automatically
> detect which kind of border color you want. Note however that you must specify **all fields** of gradient color for the configuration to be
> (re)loaded successfully

#### `border.thickness`, `border.radius`

Controls the size and corner radius of the border. Having a thickness of `0` will disable all border logic.

## Shadows

Drop shadows can be rendered behind windows. With floating windows, this becomes requires to distinguish the stacking order of windows.

#### `shadow.disable`, `shadow.floating-only`

Toggles to disable completely shadows, or only for non-floating/tiled windows. Both are `false` by default

#### `shadow.color`

Color of the shadow. You can also make use of CSS color functions to specify this. Default is fully black with opacity of `0.75`

#### `shadow.sigma`

The blur sigma of the shadow. This controls how much the shadow will "spread" below the window. Default is `10.0`

## Blur

Blur is a nice-looking effect behind semi-transparent windows. The actual implementation in the compositor is
[Dual Kawase](https://www.intel.com/content/www/us/en/developer/articles/technical/an-investigation-of-fast-real-time-gpu-based-image-blur-algorithms.html),
a fast approximation of Gaussian blur.

> [!WARNING] Blur performance
>
> While good-looking, blur is an expensive operation that can heavily tax your GPU,
> especially on lower-end systems. If you are suffering from poor performance or low battery life on
> laptops, consider disabling blur as your first measure.
>
> You can also reduce the number of `passes`, more passes means more drawing happening which means
> higher performance cost. However that can make the blur look off/not accurate with high `radius` values.
>
> Even on a high-end systems, you might want to disable the blur on your games using a
> [window rule](/configuration/window-rules) for optimal performance.

#### `blur.radius`

How much we should offset when sampling the blurred texture. In layman's terms, the higher the number the blurrier the result. Most values
above `20` will just make everything blend together

---

#### `blur.passes`

The number of passes for the blur. More blur passes are required to get correct sampling for higher radius values. They more or less correlate together,
though nothing stops you from using high number of passes with low blur values, if you care about the accuracy of the results.

#### `blur.noise`

Additional noise effect to add when rendering blur. It just looks nice and can give off the "glassy blur" effect, similar to Windows 11 Acrylic
blur look.
