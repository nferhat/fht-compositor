# Animations

`fht-compositor` has many animations to make different interactions fluid. You can fine-tune the animation curves to you liking
for snappy animations or get nice buttery smooth transitions.

## Animation curves

An animation is simply a function of time `t` that interpolates between two values: `start` and `end`. You can configure
said function to your liking.

> [!TIP]
> When deserializing the configuration, we make use of tagged unions. Depending on the value that you assign to the `curve` field, we automatically
> detect which kind of curve you want. Note however that you must specify **all fields** of for the configuration to be (re)loaded successfully

### Pre-defined easings

This is by far the simplest option. All easings on on [easings.net](https://easings.net/) are available, but rename them from
`camelCase` to `kebab-case`, for example `easeInQuint` becomes `ease-in-quint`

You must give a duration to the animation with a pre-defined easing.

```toml
[animations.window-geometry]
curve = "ease-out-quint"
duration = 450 # in milliseconds
```

### Custom cubic curve

You can also use your own easing curves in the form of a custom cubic Bezier curve. There are four control points
1. `x=0`, `y=0` (this point is forced to keep the values in bound)
2. Custom point 1, `p1`
2. Custom point 2, `p2`
4. `x=1`, `y=1` (this point is forced to keep the values in bound)

### Springs

The last kind of curves we support are spring curves. They use a physical model of a spring that is *identical* to
[libadwaita's `SpringAnimation`](https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1.3/class.SpringAnimation.html).

Since they are much more tweakable, you'd rather use something like [Elastic](https://apps.gnome.org/Elastic/) to tweak
the parameters yourself, and get a preview of what the curve will look like, as well as the total animation time.

![Elastic window](/assets/elastic.png)

Note however that you should be conservative with the values you pass into this animation, as it can *really quickly* cause
values to overshoot/undershoot to infinity, and potentially causing crashes (integer overflows).

```toml
[animations.workspace-switch]
curve = { clamp = true, damping-ratio = 1, initial-velocity = 5, mass = 1, stiffness = 700 }
# NOTE: The duration given here is not taken into account at all!
# The spring's simulation duration will be used instead
duration = 1000000000000
```

## Configuration options

#### `disable`

Disable all animations. Useful if you want the snappiest most responsive experience, of if animations
take a toll on your device performance.

> [!TIP]
> Each animation has an individual `disable` toggle!

---

#### `window-geometry`

Animation settings used for window geometry changes: both *location* and *size* are animated.

Default curve:

```toml
[animations.window-geomtry.curve]
initial-velocity = 1.0
clamp = false
mass = 1.0
damping-radio = 1.2
stiffness = 800.0
epsilon = 0.0001
```

---

#### `window-open-close`

Animation settings used for opening and closing of windows. The window open-close animation makes windows
pop in/out from their center, and fades them in/out.

Default curve:

```toml
[animations.window-open-close.curve]
initial-velocity = 1.0
clamp = false
mass = 1.0
damping-radio = 1.2
stiffness = 800.0
epsilon = 0.0001
```

---

#### `workspace-switch`

Animations settings for switching workspaces. The animation slides workspaces in/out the output's edges depending
on the workspace index relative to the one you're switching to.

`workspace-switch.direction`: Can either be `horizontal` or `vertical`

Default curve:

```toml
[animations.workspace-switch.curve]
initial-velocity = 1.0
clamp = false
mass = 0.85
damping-radio = 1.0
stiffness = 600.0
epsilon = 0.0001
```
