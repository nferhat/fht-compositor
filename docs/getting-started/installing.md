# Install guide

## Using package managers

::: tabs
== Nix (flake)

If you are using Nix(OS) with [Nix Flakes](https://nixos.wiki/wiki/flakes) enabled, refer to the [Nix](/usage/nix) page,
and the [example Nix flakes setup](./example-nix-setup)

== Arch (AUR)
```sh
paru -S fht-compositor-git
# Needed for screencast to work
paru -S fht-share-picker-git
# Recommended
paru -S uwsm
```
:::


## Manual install

First, get the latest Rust available through your preferred means.

> [!NOTE] Minimum supported rust version
> The MSRV of `fht-compositor` is mostly tied to [Smithay](https://github.com/smithay)'s MSRV.<br>
> As of writing this it's currently `1.80`

You will need the following system dependencies. Depending on your distribution, you might want to install the
`-dev` or `-devel` or `-headers` variant to get developement headers that are required to build.

- `libwayland` and dependencies
- `libxkbcommon` and dependencies
- `mesa` with the appropriate OpenGL driver for you GPU.
- In order to run the compositor from a TTY: `libudev`, `libinput`, `libgbm`, [`libseat`](https://git.sr.ht/~kennylevinsen/seatd), `libdrm` and `lib-displayinfo`
- To use [XDG screencast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html): `pipewire`, `dbus`

Then you can proceed with compiling.

```sh
# Clone and build.
git clone https://github.com/nferhat/fht-compositor/ && cd fht-compositor

# If you are not under systemd
cargo build --profile opt
# If you are under systemd, highly recommended
# See below the note on UWSM
cargo build --profile opt --features uwsm
# You can copy it to /usr/local/bin or ~/.local/bin, make sure its in $PATH though!
cp target/opt/fht-compositor /somewhere/inside/PATH

# Wayland session desktop files
install -Dm644 res/fht-compositor.desktop -t /usr/share/wayland-sessions # generic
# See below the note on UWSM, highly recommended
install -Dm644 res/fht-compositor-uwsm.desktop -t /usr/share/wayland-sessions
```

> [!CAUTION] Build features
> Do **not** compile the compositor with `--all-features` as some of these are reserved for dev/testing purposes (for exxample
> enabling profiling). Always refer to the `Cargo.toml` file before enabling features

> [!TIP] Using Universal Wayland Session Manager
> If you are using a systemd distribution, you are *highlighy* recommended to install [UWSM](https://github.com/Vladimir-csp/uwsm)
> to launch the compositor session as it will bind many systemd targets to make the overall compositor experience better.
>
> To do so, install UWSM with your favourite package manager and enable the `uwsm` feature at build time.

> [!NOTE] Portals
> Refer to the [portals](/usage/portals) page if you want the included portal (you most likely want to)
