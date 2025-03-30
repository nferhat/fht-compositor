# General behaviour configuration

#### `cursor-warps`

Whether to warp the pointer cursor whenever based on select events/actions. This currently includes

- When opening a new window, warp the pointer to the center
- When connecting a new output, warp the pointer to its center
- When using `focus-next/previous-window/output` key actions

Default is `true`

---

#### `focus-new-windows`

Whether to focus newly opened windows. Default is `true`

---

#### `focus-follows-mouse`

Whether to automatically update the keyboard focus to the window under the pointer cursor. Default is `false`

---

#### `insert-window-strategy`

How to insert a newly opened window in a workspace.

- `end-of-slave-stack`: Insert it at the end of the slave stack, IE. push it on the window list
- `replace-master`: Replace the first master window
- `after-focused`: Insert after the currently focused window

Default is `end-of-slave-stack`

---

#### `layouts`

The [dynamic layouts](/usage/layouts) to enable. You can cycle through them using the `select-next/previous-layout`
key actions, and workspaces can have different layouts active at the same time.

Available layouts are
- `tile`: A classic master-slave layout, with the master stack on the left.
- `bottom-stack`: A variant of the `tile` layout with the master stack on the upper half of the screen.
- `centered-master`: A three column layout where the master stack is centered

Default is `["tile"]`

---

#### `nmaster`, `mwfact`

The number of master clients in the workspace. Refer to the [dynamic layouts](/usage/layouts) page to understand
how they affect the layout system.

Default is `nmaster=1`, `mwfact=0.5`

> [!NOTE] Transient properties
> These layout properties are *transient* and kept around across config reloads, IE. if you changed them during
> runtime they will not be reloaded with the configuration! For example:
>
> 1. Start the compositor with `nmaster=1` and `mwfact=0.5`
> 2. On workspace 1, open some windows, change the number of master clients, make the master stack a bit wider
>    and change these values around to fit your current workflow
> 3. On workspace 2, just open two windows
> 4. Change these values in your configuration
>
> When the compositor reloads the configuration, workspace 1 `nmaster` and `mwfact` will not be reset to the
> new configuration value, since your current workflow on workspace 1 *depends* on the correct values. However,
> workspace 2 will apply the new settings, since you have not changed them when using it.
>
> This is done to avoid destroying the workflows you have set in place when reloading the configuration

---

#### `outer-gaps`, `inner-gaps`

Gaps to add around the screen edge and in between windows respectively.

Default is `8` for both.
