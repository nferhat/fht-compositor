# Portals in `fht-compositor`

[XDG desktop portals]() are a core component of any Wayland desktop session. They allow user programs and applications to
interact with other components of the system (like the compositor) in a safe and secure way through D-Bus.

The default recommended portal for `fht-compositor` desktop sessions is [xdg-desktop-portal-gtk](http://github.com/flatpak/xdg-desktop-portal-gtk),
it provides all the basics needed (file picker, accounts, settings, etc.). `gnome-keyring` can be added to have `Secrets` portal
support (needed for programs like Fractal or Secrets)

In addition, `fht-compositor` provides additional portals that need a session-specific implementation.

## XDG ScreenCast portal

The XDG screencast portal is used by applications that request casting/recording a screen or part of a screen. Example
programs include [OBS](https://obsproject.com/download) and web browsers (through WebRTC).

You can chose between three options:

- Screencast an entire monitor
- Screencast a workspace: It will include only the workspace windows, not any layer shells
- Screencast a window: The window itself with additional popups will be screencasted

The screencasts are **damage-tracked**, IE. new frames will be pushed and drawed only when *something* changes. Moreover,
only DMABUF-based screencasting is supported, as SHM is way too slow. If that is needed, use the `wlr-screencopy` protocol.
