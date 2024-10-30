# Installing

The current way to get `fht-compositor` up and running on your system is compile it yourself on your machine.

## Dependencies

First, install [`rustup`](https://rustup.rs) to get the latest rust version
> **NOTE**: Current MSRV is `1.80`

Then, install the following dependencies, note that on some distros (like Debian-based ones) you might need to install the `-dev`/`-devel` variant to get developement headers

- `libwayland` and dependencies
- `libxkbcommon` and dependencies
- `mesa` with the appropriate mesa OpenGL driver for your GPU.
- To run `udev` backend inside a libseat session:
  - `libudev`
  - `libinput`
  - `libgbm`
  - [`libseat`](https://git.sr.ht/~kennylevinsen/seatd)
  - `libdrm`
  - `lib-displayinfo`
- To run the `X11` backend inside an X11 window:
  - `libxcb`
  - `libXau`
  - `libXdmcp`
- To use the [XDG screencast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
  - `pipewire`
  - `dbus`

## Compiling and installing

```sh
git clone https://github.com/nferhat/fht-compositor/ && cd fht-compositor
cargo build --no-default-features --features=udev-backend --profile opt
cp target/opt/fht-compositor /somewhere/inside/PATH
```

If you want to use the XDG screencast portal, install the portal configuration file, and compile the compositor with the required features:

> **NOTE**: Do *not* compile the compositor with `--all-features` as some of these are for developement purposes only! (for example, enabling profiling). 
>
> Always refer to the `Cargo.toml` file before enabling features.

```sh
cargo build --no-default-features --features=udev-backend,xdg-screencast-portal --profile opt
mkdir -p $XDG_CONFIG_HOME/xdg-desktop-portal/
cp res/fht-compositor.portal $XDG_CONFIG_HOME/xdg-desktop-portal/portals/
```

In addition, you will need to get [`fht-share-picker`](https://github.com/nferhat/fht-share-picker)  in order to get the XDG screencast portal working

```sh
git clone https://github.com/nferhat/fht-share-picker/ && cd fht-share-picker
cargo build --profile opt
cp target/opt/fht-share-picker /somewhere/inside/PATH
```

## Running

You can now run `fht-compositor` from your TTY (`udev` backend) or inside a parent compositor to try it out (`X11` backend), though the latter option is mostly here for developement purposes.

```sh
fht-compositor
# You need to start a dbus session to get portals working
dbus-run-session fht-compositor
```

You need to make sure that `pipewire` starts *before* you use the program that requires the XDG screencast portal (for example OBS). This is achieved by putting the following inside of your configuration

```toml
autostart = [
  "your-distro-pipewire-starter",
  # Example for gentoo's pipewire setup
  "gentoo-pipewire-launcher",
  # Or, you can use something like DeX to autostart all XDG programs
  "dex -a",
]
```
