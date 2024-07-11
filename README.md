# fht-compositor

`fht-compositor` is a Wayland compositor that aims to have simple window management that is easy to
understand and get accommodated to, inspired by X11 window managers, mainly DWM and AwesomeWM.

Every output has its workspace set, each constituting of 9 workspaces. When using keybinds,
their effects are scoped to the active output, and if needed, the active workspace, and the active
workspace window.

Each workspace provides a *dynamic tiling* layout (currently supporting bottom stack and tile), with
a system of "tiles" that contain windows.


https://github.com/nferhat/fht-compositor/assets/104871514/b5484cb5-65e8-4936-85a9-0e21d72b0254
> preview video, showcasing some features of the compositor

## Features

- Can be ran user an X11 window, or under a TTY.
- Workspaces with dynamic layout system
- Some basic animations .
- Window borders, with rounded corners support
- Configuration:
- Input configuration (global and per-device), with both keybinds and mousebinds.
- Window rules (based on title/app_id/current workspace/etc.)
- Output Screencast/Screen recording support through the XDG ScreenCast portal interface.

## TO-DOs

- Xwayland support (very unlikely, but you can use a Xwayland rootful window)
- Session lock support

## Install

1. Main compositor

```sh
cargo build --profile opt
# Or, if you want to customize features (see Cargo.toml)
cargo build --no-default-features --features=egl,udev_backend --profile opt
cp target/opt/fht-compositor /somewhere/inside/path

# If you are going to use freedesktop XDG portals.
# You can also configure portals.conf(5) (see man page), but it should work by default.
cp res/fht-compositor.portal $XDG_CONFIG_HOME/xdg-desktop-portal/portals/
```

2. [`fht-share-picker`](https://github.com/nferhat/fht-share-picker)

```sh
# To select the desired output to screencast
git clone https://github.com/nferhat/fht-share-picker
cargo build --profile opt
cp target/opt/fht-share-picker somewhere/inside/path
```

## Running

The compositor should run fine, and install a default configuration and greet you! (hopefully) If not, install a configuration yourself to `$XDG_CONFIG_HOME/fht/compositor.ron`

- The compositor *is a portal*, by that I mean that there's no other process to start up in order to get them to work, since the compositor exposes itself the required D-Bus apis to run them.

- You should run the compositor using `dbus-run-session fht-compositor`, and not `fht-compositor` only, since it assumes there's always a D-Bus session set up to run the compositor.

- Please take into account that this is still an alpha-quality piece of software at best, there's still a [LOT to do](https://github.com/nferhat/fht-compositor/issues/2) to reach a "stable" release. Any kind of feedback/help would be appreciated.
