# Output configuration

When starting up, `fht-compositor` will scan all connected outputs and turn them on. They will get arranged
in a straight horizontal line, like the following:

![Default output arrangement](/assets/default-output-arrangement.svg)

You refer by outputs using their connector names, for example `eDP-1` is your laptop builtin display,
`DP-{number}` are display port connectors, `HDMI-A-{number}` are builtin HDMI ports, etc.

You configure outputs by using the `outputs.{connector-name}` table.

> [!NOTE] Output management tools
> The compositor supports the `wlr-output-management-v1` protool, allowing you to use tools like [wlr-randr](https://sr.ht/~emersion/wlr-randr/)
> or any GUI equivalent to manage the outputs at runtime. Be aware that the output configuration will be *reset* if you change the
> output configuration!

---

#### `disable`

Whether to completely disable an output. You will not be able to accesss it using the mouse.

> [!NOTE] Disabling an already enabled output
> When you disable a output that has opened windows in its workspaces, these windows will get "merged" or into the same workspaces
> of the *newly active* output instead.

---

#### `mode`

A string representation of a mode that takes the form of `{width}x{height}` or `{width}x{height}@{refresh-hz}`. Optionally, there's
custom mode support using [CVT timing calculation](http://www.uruk.org/~erich/projects/cvt/)

When picking the mode for the output, the compositor will first filter out modes with matching width and height, then
- If there's a given refresh rate, find the mode which refresh rate is the closest to what you have given
- If there's no refresh rate, pick the highest one available.

---

#### `scale`

The *integer scale* of the output. There's currently no support for fractional scaling in `fht-compositor`.

---

#### `position.x`, `position.y`

The position of the *top-left corner*. The output space is absolute.

> [!NOTE] Overlapping output geometries
> If your configuration contains two overlapping outputs, `fht-compositor` will resort to the default output arragement seen
> at the top of this page. It will also print out a warning message in the logs

---

#### `vrr`

Whether to enable Variable Refresh Rate mode. This is called NVidia GSync or AMD FreeSync by manufacturers. In short,
it allows the display to adapt its refresh rate to the content being displayed, allowing for smooth presentation.

It's very useful in games for example, where you can't reach your target display FPS.

There are three modes available:
- `"on"`: Always enable VRR
- `"off"`: Always disable VRR
- `"on-demand"`: Enable VRR only if
  * A window has been marked with the rule [vrr](/configuration/window-rules#vrr)
  * The window is scanned out to the primary plane (which most likely means having
  the window fullscreen with no other layer-shells displayed above it)

---

#### `transform`

Option to rotate your display

Following options are availible:
- `"normal"`: Keep default roation
- `"90"`: Rotate 90°
- `"180"`: Rotate 180°
- `"270"`: Rotate 270°
- `"flipped"`: Flip the output
- `"flipped-90"`: Flip the output and rotate 90°
- `"flipped-180"`: Flip the output and rotate 180°
- `"flipped-270"`: Flip the output and rotate 270°