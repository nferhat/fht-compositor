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
> As of writing this it's currently `1.85.1`

You will need the following system dependencies. Depending on your distribution, you might want to install the
`-dev` or `-devel` or `-headers` variant to get developement headers that are required to build.

- `libwayland` and dependencies
- `libxkbcommon` and dependencies
- `mesa` with the appropriate OpenGL driver for you GPU.
- In order to run the compositor from a TTY: `libudev`, `libinput`, `libgbm`, [`libseat`](https://git.sr.ht/~kennylevinsen/seatd), `libdrm` and `lib-displayinfo>=0.3.0`
- To use [XDG screencast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html): `pipewire`, `dbus`

Then you can proceed with compiling.

```sh
# Clone and build.
git clone https://github.com/nferhat/fht-compositor/ && cd fht-compositor

# If you are not under systemd
cargo build --profile opt
# Integrate with systemd for user session
cargo build --profile opt --features systemd
# You can copy it to /usr/local/bin or ~/.local/bin, make sure its in $PATH though!
cp target/opt/fht-compositor /somewhere/inside/PATH
```

Now install the required files in the appropriate locations

::: tabs
== With Systemd
```sh
install -Dm755 res/systemd/fht-compositor-session         -t /somewhere/inside/PATH
install -Dm644 res/systemd/fht-compositor.desktop         -t /usr/share/wayland-sessions
install -Dm644 res/systemd/fht-compositor.service         -t /etc/systemd/user
install -Dm644 res/systemd/fht-compositor-shutdown.target -t /etc/systemd/user
```

== Generic (without systemd)
```sh
install -Dm644 res/fht-compositor.desktop -t /usr/share/wayland-sessions
```

:::

> [!CAUTION] Build features
> Do **not** compile the compositor with `--all-features` as some of these are reserved for dev/testing purposes (for exxample
> enabling profiling). Always refer to the `Cargo.toml` file before enabling features

> [!NOTE] Portals
> Refer to the [portals](/usage/portals) page if you want the included portal (you most likely want to)
