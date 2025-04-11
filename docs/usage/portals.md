# Portals in `fht-compositor`

[XDG desktop portals]() are a core component of any Wayland desktop session. They allow user programs and applications to
interact with other components of the system (like the compositor) in a safe and secure way through D-Bus.

The default recommended portal for `fht-compositor` desktop sessions is [xdg-desktop-portal-gtk](http://github.com/flatpak/xdg-desktop-portal-gtk),
it provides all the basics needed (file picker, accounts, settings, etc.). `gnome-keyring` can be added to have `Secrets` portal
support (needed for programs like Fractal or Secrets)

In addition, `fht-compositor` provides additional portals that need a session-specific implementation.

## Setting up `fht-compositor` XDG desktop portal.

> [!TIP] AUR package/Nix module
> - If you are using `fht-compositor` through the NixOS/home-manager modules or the `fht-compositor` package, this should
> have already been sorted out for you!
> - If you are using the AUR package, this should also be sorted out for you!

Install the following files in the relevant places

| File                                     | Target path                              |
| ---------------------------------------- | ---------------------------------------- |
| `res/fht-compositor.portal`              | `/usr/share/xdg-desktop-portal/portals/` |
| `res/fht-compositor-portals.conf`        | `/usr/share/xdg-desktop-portal/`         |

The compositor itself should start up the portal after initializing graphics (IE starting up outputs).

## XDG ScreenCast portal

The XDG screencast portal is used by applications that request casting/recording a screen or part of a screen. Example
programs include [OBS](https://obsproject.com/download) and web browsers (through WebRTC).

You can chose between three options:

- Screencast an entire monitor
- Screencast a workspace: It will include only the workspace windows, not any layer shells
- Screencast a window: The window itself with additional popups will be screencasted

The screencasts are **damage-tracked**, IE. new frames will be pushed and drawed only when *something* changes. Moreover,
only DMABUF-based screencasting is supported, as SHM is way too slow. If that is needed, use the `wlr-screencopy` protocol.

Using this portal will require an additional dependency, [`fht-share-picker`](https://github.com/nferhat/fht-share-picker),
it is **required** to select which source (from the above) to screen cast.

::: tabs
=== Nix
If you are using the [Nix module](/usage/nix), this should be installed if you compile `fht-compositor` with default attrs.
Otherwise, make sure to use a package with `withXdgScreenCast` set to true, like so:

```nix
fht-compositor.override { withXdgScreenCast = true; /* other attrs */ }
```
=== Arch (AUR)
```bash
# Or use your AUR helper of choice.
paru -S fht-share-picker-git
```
=== From source
```bash
git clone https://github.com/nferhat/fht-share-picker && cd fht-share-picker
cargo build --release
# You can copy it to /usr/local/bin or ~/.local/bin
cp target/release/fht-share-picker /somewhere/in/PATH
```
:::
