# Mousebindings

These are exactly the same as [keybindings](/configuration/keybindings), expect the available mouse buttonss (instead of
key names) are: `left`, `right`, `middle`, `right`, `forward`, `back`, `scrollup` (or `wheelup`), `scrolldown` (or `wheeldown`), `scrollleft` (or `wheelleft`), `scrollright` (or `wheelright`)

## Available mouse actions

- `swap-tile`: Initiates an interactive tile swap. Allows you to grab a window and put it elsewhere in the window stack,
  move it across outputs or workspaces. For floating windows, it allows you to move them around with the mouse
- `resize-tile`: Initiates an interactive resize. Only affects floating windows so far.
- `focus-next-window`, `focus-previous-window`: Focused the next/previous window in the workspace. Removes the currently active fullscreen window, if any.
- `focus-next-workspace`, `focus-previous-workspace`, I think these are clear.