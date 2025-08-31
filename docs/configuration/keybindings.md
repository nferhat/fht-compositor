# Keybindings

Keybindings allow you to execute an action after hitting a specific key pattern. The actions can interact with the internal
state of the compositor (nmaster, master width factor, focused window/workspace, etc.), or not (execute a command line,
for example)

Keybindings are defined in a table, the keys are the key pattern, and values are the actions to execute

```toml
[keybinds]
Super-Return = { action = "run-command", arg = "ghostty" }
Super-Shift-j = "swap-with-next-window"
```

> [!WARNING] Default keybindings
> There are no default keybindings, you should always keep a key bind to `quit` and `reload-config`, otherwise, you
> might get stuck in the compositor!

## Key patterns

A key pattern describes a specific key combo. It is composed from the following parts, joined by hyphens/dashes (`-`):

- A set of modifier keys:
  - `Mod`/`Super`/`Logo`/`Meta`/`M`: Usually the left super key, has the Windows logo on it
  - `Ctrl`/`Control`/`C`: Left/Right Control key, usually marked with `Ctrl`
  - `Shift`/`S`: Left/Right Shift key
  - `Alt`/`A`: Left/Right Alt key
  - `AltGr`: AltGr key, or named sometimes `ISO_Level3_Shift`, only available in some layouts (for example French)
- A key name on your keyboard.

> [!TIP]
> You can get key names using a program like [wev](https://git.sr.ht/~sircmpwn/wev)
>
> ```
> [14:     wl_keyboard] key: serial: 1383022871; time: 10735614; key: 36; state: 0 (released)
>                       sym: Return       (65293), utf8: ''
> [14:     wl_keyboard] key: serial: 1383333097; time: 10738088; key: 38; state: 1 (pressed)
>                       sym: a            (97), utf8: 'a'
> [14:     wl_keyboard] key: serial: 1383341414; time: 10738153; key: 38; state: 0 (released)
>                       sym: a            (97), utf8: ''
> [14:     wl_keyboard] key: serial: 1383371466; time: 10738387; key: 113; state: 1 (pressed)
>                       sym: Left         (65361), utf8: ''
> [14:     wl_keyboard] key: serial: 1383379644; time: 10738450; key: 113; state: 0 (released)
>                       sym: Left         (65361), utf8: ''
> [14:     wl_keyboard] key: serial: 1383608097; time: 10740288; key: 47; state: 1 (pressed)
>                       sym: semicolon    (59), utf8: ';'
> [14:     wl_keyboard] key: serial: 1383611141; time: 10740312; key: 47; state: 0 (released)
>                       sym: semicolon    (59), utf8: ''
> [14:     wl_keyboard] key: serial: 1383692118; time: 10740943; key: 20; state: 1 (pressed)
>                       sym: minus        (45), utf8: '-'
> [14:     wl_keyboard] key: serial: 1383694914; time: 10740965; key: 20; state: 0 (released)
>                       sym: minus        (45), utf8: ''
> [13:      wl_pointer] motion: time: 10741546; x, y: 617.093750, 222.000000
> ```
>
> The key name is just after `sym`, for example `Return`, `Left`, `minus`. Capitalization should not matter.

> [!NOTE] Resolving key names
> When resolving keys, `fht-compositor` will use the key names emitted from the *active layout*, not the raw key. So
> keybinds working on a layout might *not* work in another!

## Key action settings

You can tweak the behaviour of a key action using the following settings.

---

#### `allow-while-locked`

Whether to allow the key bind to execute when there's an active session lock. By default, `fht-compositor` disable all keybinds
when a session locker starts. You can override this like so:

```toml
[keybinds.XF86AudioLowerVolume]
action = "run-command"
arg = "wpctl set-volume -l 1 @DEFAULT_AUDIO_SINK@ 10%-"
allow-while-locked = true # allow volume control while locked
```

---

#### `repeat`

Whether to repeat this key action. Repeat rate is the same as the globally configured one in the
[`input.keyboard.repeat-rate`](/configuration/input#repeat-rate-repeat-delay).

```toml
[keybinds.Super-Up]
action = "move-floating-window"
arg = [0, -50]
repeat = true
```

## Available key actions

#### `quit`

Exits the compositor

---

#### `reload-config`

Reload compositor configuration. You should have to-do this, since the compositor automatically reloads it.

---

#### `run-command`

Runs an arbitrary command line. Evaluated with `/bin/sh -c "<command line>"`. Example:

```toml
[keybinds]
Super-Return = { action = "run-command", arg = "ghostty" }
```

---

#### `select-next/previous-layout`

Selects the next or previous layout available. See [`general.layouts`](/configuration/general#layouts) and [dynamic layouts](/usage/layouts)
pages for more information.

---

#### `change-nmaster`, `change-mwfact`, `change-proportion`

Changes each of these parameters, `change-proportion` for the active window. See [dynamic layouts](/usage/layouts) page.

---

#### `close-focused-window`

Closes the currently focused window

---

#### `maximize-focused-window`, `fullscreen-focused-window`, `float-focused-window`

Toggle each of these states on the focused window.

---

#### `center-floating-window`

Centers the currently focused floating window. If the window is not floating, nothing happens

---

#### `move-floating-window`, `resize-floating-window`

Moves or resizes the currently focused window. Takes argument in the form of `[dx, dy]` or `[dw, dh]`, example:

```toml
[keybinds]
Super-Left  = { action = "move-floating-window", arg = [-10, 0] }
Super-Right = { action = "move-floating-window", arg = [10 , 0] }
Super-Up    = { action = "move-floating-window", arg = [0,  10] }
Super-Down  = { action = "move-floating-window", arg = [0, -10] }
```

---

#### `focus-next-window`, `focus-previous-window`

Focused the next/previous window in the workspace. Removes the currently active fullscreen window, if any.

---

#### `swap-with-next-window`, `swap-with-previous-window`

Swaps the currently focused window with next/previous window in the workspace. Removes the currently active fullscreen window, if any.

---

#### `focus-next-output`, `focus-previous-output`

Focus the next/previous output in the global space. Outputs are ordered by the way they got detected/inserted.

---

#### `focus-workspace`, `send-focused-window-to-workspace`

Key actions to focus or send the focused window to a workspace. Takes in the workspace **index**

```toml
Super-1 = { action = "focus-workspace", arg = 0 }
Super-Shift-1 = { action = "send-focused-window-to-workspace", arg = 0 }
```

---

#### `focus-next-workspace`, `focus-previous-workspace`,

I think these are clear.

---

#### `None`

Used to disable a specific key pattern.
