# Recommended software

`fht-compositor` is not a complete desktop environment/suite, so it is up to you, the end user, to install some basics in order to get your Wayland session to work properly:

- Pipewire: other than for the [XDG screencast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html), you would want to have it in order to have sound in your session. Refer to your distro instructions for setup.

- A notification daemon: for example [mako](https://wayland.emersion.fr/mako/), start it using `autostart` field. Some programs that require a notification daemon present will freeze if its not started up (for example Discord)

- Clipboard manager: Required to keep clipboard data after an application closes, for example [clipman](https://github.com/chmouel/clipman) or [cliphist](https://github.com/sentriz/cliphist), note that all of these require, `wl-clipboard` package to be installed on your system.

- An application launcher: for example [wofi](https://github.com/lbonn/rofi), [bemenu](https://github.com/Cloudef/bemenu), or [anyrun](https://github.com/anyrun-org/anyrun), required to launch applications from `.desktop` files, or programs in `$PATH`

- A polkit agent: packages such as `polkit-gnome`, `polkit-plasma`, start it using `autostart` field. Required when programs ask for privileges using a popup with a password.
