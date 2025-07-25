# Window rules

Window rules are a simple but powerful way to streamline and organize your workflow. They pair really well with
the [static workspaces](/usage/workspaces) system that `fht-compositor` has. You can make a window rule target
a specific set of windows by filtering (or *matching*) properties that you want, like app identifier, title,
etc.

A window rule has two parts: the match part, and the properties part.

## The match part

A window rule can have multiple criteria to scope its requirements. For example, the following rule will
only be applied to Alacritty windows, and will make it open maximized.

```toml
[[rules]]
match-app-id = ["Alacritty"]
maximized = true
```

You can have different matching requirements, the following rule matches windows Alacritty windows *or*
LibreWolf windows

```toml
[[rules]]
match-app-id = ["Alacritty", "LibreWolf"]
maximized = true
```

<!-- FIXME: Use the IPC in order to get title and app-id
    There will be an action called `pick-window` or `select-window`
    Using it you can get the window ID under the cursor. -->

---

#### `match-all`

When a window rule is marked as `match-all`, all the given match requirements *must* match. For example,
this window rule will only match Fractal windows that are on workspace with the index 2

```toml
[[rules]]
match-all = true
match-app-id = ["org.gnome.Fractal"]
on-workspace = 2
```

However, this window rule matches Fractal windows *or* all windows opened on workspace with index 2

```toml
[[rules]]
match-app-id = ["org.gnome.Fractal"]
on-workspace = 2
```

This allows you to make very precise window rules.

> [!TIP] Different matching rules
> If different rules match onto the same window, they will "merge" together, based on their declaration order.
> For example, having these two window rules
>
> ```toml
> [[rules]]
> match-app-id = ["Alacritty"]
> floating = true
>
> [[rules]]
> match-app-id = ["Alacritty"] # etc, imagine its just another rule
> floating = false
> ```
>
> Will result in a window rule with `floating` equal to **false** (since it was declared *later*) in your config.
> This is why you should try to make your matching conditions precise to avoid unexpected things to happen!

---

#### `match-title`, `match-app-id`

Both are a list of [Regular Expression](https://en.wikipedia.org/wiki/Regular_expression)s. This match conditions
requires that the window matches only *one* regex from the given regexes.

The following window rule matches WebCord, Telegram, and Fractal windows

```toml
[[rules]]
match-app-id = ["WebCord", "Telegram", "org.gnome.Fractal"]
open-on-workspace = 2

```

This window rule, matches all Steam games that are opened with Proton

```toml
[[rules]]
match-app-id = ["steam_app_*"]
floating = true
centered = true
on-workspace = 5
```

---

#### `on-output`

Match on the output the window is opened on. Nothing fancy.

The following rule matches all windows opened on a laptop's internal display

```toml
[[rules]]
on-output = "eDP-1"
floating = true
```

---

#### `on-workspace`

Match on the workspace *index* (not number!) the window is opened on.

---

#### `is-focused`

Match on the currently focused window. A focused window is the one that should receive keyboard focus.
There can be *multiple* focused window, one per workspace.

The following rule matches all *un*focused windows

```toml
[[rules]]
is-focused = false
opacity = 0.95 # lesser visible non-focused windows
```

---

#### `is-floating`

Match on the floating window(s). This rule does not match all the windows if the workspace layout is
set to floating, instead, it matches depending on the [`floating`](#maximized-fullscreen-floating-centered)
property, or when you float a window using the
[`float-focused-window`](/configuration/keybindings#maximize-focused-window-fullscreen-focused-window-float-focused-window)

## Window properties

#### `open-on-output`, `open-on-workspace`

These properties control *where* the window should open. They take an output name and a workspace *index*

If the given output/workspace index is invalid, the compositor will fallback to the active one. (This is the
default when no window rule is applied)

The following rule opens several game launchers/games on workspace with the index 5

```toml
[[rules]]
match-app-id = [
  "Celeste.bin.x86_64",
  "steam_app_*",
  "Etterna",
  "Quaver",
  "Steam",
  "org.prismlauncher.PrismLauncher"
]
open-on-workspace = 5
centered = true
floating = true
```

---

#### `border`, `blur`, `shadow`

These values take the same fields as their versions in the [decorations configuration](/configuration/decorations),
however, they will *override* the decorations configuration with whatever fields you have provided.

For example, this window rule will disable all blur in workspace with index 5

```toml
[[rules]]
on-workspace = 5
blur.disable = true
```

---

#### `proportion`

Change the initial window proportion. See [dynamic layouts](/usage/layouts) for information about how window
proportions affect layouts.

----

#### `opacity`

The opacity of a window, `0.0` is fully transparent, `1.0` is fully opaque.

----

#### `decoration-mode`

The decoration mode for this window. See [`decorations.decorations-mode`](/configuration/decorations#decorations-mode)
for more information about differences between these values.

Useful when a client misbehaves when using specifically SSD/CSD.

----

#### `maximized`, `fullscreen`, `floating`, `centered`

State to toggle on when opening the window. This only gets applied *once*!

They are self-explainatory. Example window rule making all GNOME apps open floating centered.

```toml
[[rules]]
centered = true
floating = true
match-app-id = ["^(org.gnome.*)$"]
```

---

#### `ontop`

Whether to place this window above all other windows. This only applies for floating windows.

```toml
[[rules]]
is-floating = true
```

---

#### `vrr`

Whether this window can trigger on-demand [VRR](/configuration/outputs#vrr). This window rule will
only trigger if the window is scanned out on the primary plane (which most likely means the window
is fullscreened)
