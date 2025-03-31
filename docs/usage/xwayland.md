# XWayland support

`fht-compositor` does **not** have native XWayland support, since X11 is very quirky and weird to
implement, and while it would play nice with the layout system (allowing for freely moving windows),
I do not have planned XWayland support.

Don't fret, however, there are still solutions if you rely on X11-only programs (which I do)

## xwayland-satellite

> [!TIP] Example setup
> The [nix example setup](/getting-started/example-nix-setup) has xwayland-satellite enabled as a user service.

[xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite) is an external XWayland
implementation that grants any compositor rootless XWayland support. X11 windows opened through
XWayland appear as normal windows and they will automatically share clipboard and render fine.

Install it through your package manager or build it from source and either add it to your `autostart` section,
or integrate it in your desktop session using systemd user services (recommended). You should find in the logs
the display number that it connected to (most likely `:0`, but you can give it a specific one to connect to, like
`xwayland-satellite :727`)

You can now use X11 programs by setting the `DISPLAY` environment variable

::: tabs
== Per-program
```sh
# For example, run steam. Flag is needed otherwise you get a black screen.
env DISPLAY=:0 steam -system-composer
```

== Setting in config
```toml
# You should force it to use a display number for this to work properly!
env.DISPLAY = ":727"
```

:::

And voila!

![X11 programs under fht-compositor](/assets/xwayland.png)

---

Other tips and information to keep in mind are:

- Video games and simple clients open and work 100% fine! No performance issue is noticed, a seamless experience.
- You should use the `floating` layouts if you plan on using X11 applications since they sometimes don't play nice with dyanmic
  layouts.
- If you use self-resizing/self-moving windows, they will not play very nice too!

## Rootful XWayland

Sometimes, programs just don't play well with xwayland-satellite, so you must resort to using rootful XWayland, in order words:
run XWayland inside a window.

1. Install `xwayland` using your system package manager
2. Install a simple X11 window manager of your choice, recommended is [i3wm](https://i3wm.org).
3. Run XWayland and your window manager inside, and open a terminal.

This approach comes with many downsides, notably the fact your X11 windows live *inside* another window, which can hurt your
workflow a little bit. You can however fullscreen the XWayland window and enable keyboard grabbing to get a seamless X11 session,
in case you need it.

The other downside is that the clipboard is **not** shared. You can use X11 clipboard tools like `xset` to pull/push values
from/to the X11 clipboard.

```sh
env DISPLAY=:0 xsel -ob | wl-copy     # Get a value from XWayland
wl-paste -n | env DISPLAY=:0 xsel -ib # Push a value to XWayland
```
