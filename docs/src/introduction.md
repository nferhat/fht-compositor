# Introduction

`fht-compositor` is a wayland compositor that implements a static workspace
management system: Each output gets a static number of workspaces, and each
workspace contains a number of windows.

Windows get arranged in two stacks: a master stack and a slave stack.

Workspaces implement *dynamic tiling*, using a predefined layout and some
parameters, all the windows in the workspace get tiled to fit inside the
output space in order to waste as little screen space as possible.

At runtime, the user can change and tweak said parameters of layouts: the
number clients in the master stack, the proportion of the master stack, etc...
using various keybinds and actions.

## Features

- `X11` backend (run inside an X11 window) or `udev` backend (libseat session)
- Static workspace system as described above
- Some cool animations!
- `toml` configuration
- [XDG screencast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
  (output mode only)
