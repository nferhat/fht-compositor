# Gesture bindings

Gesture bindings allow you to bind actions to multi-finger swipe gestures on touchpads. The gestures are recognized globally, meaning they will work regardless of which window is focused.

## gesture patterns

`action = { fingers = finger_numbers, direction = swipe_direction, min_swipe_distance = distance}`: A gesture with `finger_numbers` fingers swiping in `swipe_direction` to make the `action`. The `min_swipe_distance` is optional and specifies the minimum distance (arbitrary units) the fingers must move to trigger the action.

example:

```toml
[gesturebinds]
focus-next-workspace = { fingers = 4, direction = "left", min-swipe-distance = 1 }
focus-previous-workspace = { fingers = 4, direction = "right", min-swipe-distance = 1 }
fullscreen-focused-window = { fingers = 4, direction = "up", min-swipe-distance = 3 }
float-focused-window = { fingers = 4, direction = "down", min-swipe-distance = 5 }
```

## Available gesture actions
- `close-focused-window`: Closes the currently focused window.
- `float-focused-window`: Toggles the focused window between tiled and floating mode.
- `maximize-focused-window`: Toggles the focused window between maximized and normal state.
- `fullscreen-focused-window`: Toggles the focused window between fullscreen and normal state.
- `focus-next-window`: Focuses the next window in the current workspace.
- `focus-previous-window`: Focuses the previous window in the current workspace.
- `swap-with-next-window`: Swaps the focused window with the next window in the current workspace.
- `swap-with-previous-window`: Swaps the focused window with the previous window in the current workspace.
- `focus-next-workspace`: Switches to the next workspace.
- `focus-previous-workspace`: Switches to the previous workspace.
- `focus-next-output`: Focuses the next output (monitor).
- `focus-previous-output`: Focuses the previous output (monitor).
- `select-next-layout`: Switches to the next layout in the current workspace.
- `select-previous-layout`: Switches to the previous layout in the current workspace.