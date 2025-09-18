# Gesture bindings

Gesture bindings allow you to bind actions to multi-finger swipe gestures on touchpads. The gestures are recognized globally, meaning they will work regardless of which window is focused.

## gesture patterns

`finger_numbers-direction = "action"`: A gesture with `finger_numbers` fingers swiping in `direction` to make the `action`. For example, `3-left` means a three-finger swipe upwards.

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