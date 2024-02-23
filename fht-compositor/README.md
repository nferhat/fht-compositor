# fht-compositor

A wayland compositor written in rust using the Smithay crate.

## Supported features

- Running either under an X11 window under another display server, or from a tty directly (auto-detection supported)
- Basic protocols for your day-to-day desktop experience like xdg_wm_base/layer_shell/popup protocol
- Configuration file, while checking for errors and showing you an error notification.
    - Autostart programs (using /bin/sh by default)
    - Keybinding and mousebinding configuration
    - Input configuration, + per device input configuration
    - Decorations (currently only border around windows)
- Cursor rendering with custom theme and size.
- Static/per output workspace support with bottom stack and master/tile layouts.
