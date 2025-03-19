# Install guide

> The only maintained package right now is the Nix one! If you want to package `fht-compositor` for
> your distribution, feel free to open a [pull request](https://github.com/nferhat/fht-compositor/pull/new)!

## Nix install

You can go to the [Nix](../nix/flake.md) page for more information.

## Arch Linux install

[fht-compositor](https://aur.archlinux.org/packages/fht-compositor) is available on the AUR.
It can be installed using an AUR helper (e.g. `paru`):

```sh
paru -S fht-compositor
```

## Manual install

First, install [`rustup`](https://rustup.rs) to get the latest rust version

> **NOTE**: Current MSRV is `1.80`

Then, install the following dependencies, note that on some distros (like Debian-based ones) you
might need to install the `-dev`/`-devel` variant to get developement headers

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
- To run the `winit` backend:
  - `libxcb`, `libXau` `libXdmcp` (if you are under X11)
  - `libwayland` (if you are under Wayland)
- To use the [XDG screencast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
  - `pipewire`
  - `dbus`

### Compiling and installing

```sh
# Clone and build.
git clone https://github.com/nferhat/fht-compositor/ && cd fht-compositor

# If you are not under systemd
cargo build --profile opt
# If you are under systemd, highly recommended
# See below the note on UWSM
cargo build --profile opt --features uwsm


cp target/opt/fht-compositor /somewhere/inside/PATH

# Wayland session desktop files
install -Dm644 res/fht-compositor.desktop -t /usr/share/wayland-sessions # generic

# See below the note on UWSM, highly recommended
install -Dm644 res/fht-compositor-uwsm.desktop -t /usr/share/wayland-sessions

# Install the portal files, if you build with xdg-screencast
install -Dm644 res/fht-compositor.portal -t /usr/share/xdg-desktop-portal/portals
install -Dm644 res/fht-compositor-portals.conf -t /usr/share/xdg-desktop-portal
```

> **NOTE**: Do _not_ compile the compositor with `--all-features` as some of these are for
> developement purposes only! (for example, enabling profiling).
>
> Always refer to the `Cargo.toml` file before enabling features.

> **Note on using UWSM**
>
> If you are using a systemd distribution, you are _highly recommended_ to install
> [UWSM](https://github.com/Vladimir-csp/uwsm) to launch the compositor as it will
> bind many systemd targets to make the overall compositor experience better.
>
> To do so, install UWSM and build the compositor with the `uwsm` feature enabled

In addition, you will need to get [`fht-share-picker`](https://github.com/nferhat/fht-share-picker)
in order to get the XDG screencast portal working

```sh
git clone https://github.com/nferhat/fht-share-picker/ && cd fht-share-picker
cargo build --profile opt
cp target/opt/fht-share-picker /somewhere/inside/PATH
```

Additionally, install [Alacritty](https://github.com/alacritty/alacritty) and
[wofi](https://hg.sr.ht/~scoopta/wofi) to get started with the default configuration
