# `general` section

- `general.cursor-warps`

Should the compositor warp the cursor to areas in response to events, for example when focusing a window, the cursor warps to the center of a window,or when an output gets added, etc...

Default is `true`

------

- `general.focus-new-windows`

Should the compositor focus new windows when they get inserted into workspaces?

Default is `true`

------

- `general.insert-window-strategy`

How should a workspace insert a new window into its window list.

| Value | Description |
| - | - |
| `end-of-slave-stack` | Puts the window at the end of the slave-stack, pushes it at the end
| `replace-master`     | Inserts the window at the place of the master one, pushing the master window to the next position
| `after-focused`      | Inserts the window just after the focused window

Default is: `end-of-slave-stack`

------

- `general.layouts`

A list of layouts that drive the workspaces tiling algorithm.

| Value | Description |
| - | - |
| `tile`           | A classic master-stack layout, as implemented in DWM. |
| `bottom-stack`   | Similar to the tile layout, but the slave stack is at the bottom of the screen |
| `centered-master`| A layout that centers the master stack on screen, using, with the slave windows windows distributed to the left and right. |
| `floating`       | Disable tiling alltogether.  |

Default is `["tile", "floating"]`

------

- `general.nmaster`

The number of clients in the master stack

Must be greater than 0, otherwise its considered an error.

Default is `1`

------

- `general.mwfact`

The proportion of the master stack relative to the slave stack, after removing screen gaps.

This must be contained in the range `[0.01, 0.99]`

Default is `0.5`

------

- `general.outer-gaps`

The gaps applied to edge of the screen, acting as a "useless" gap for aesthetic reasons.

This can be negative, making the windows overflow outside the screen.

Default is `8`

------

- `general.inner-gaps`

The gaps applied between all the tiled windows, acting as a "useless" gap for aesthetic reasons.

This can be negative, making the windows overflow outside the screen.

Default is `8`
