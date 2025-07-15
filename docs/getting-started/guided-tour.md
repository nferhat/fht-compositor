# Guided tour of `fht-compositor`

Before launching the compositor, you are recommended to install [Alacritty](https://alacritty.org) and
[wofi](https://hg.sr.ht/~scoopta/wofi) since the default configuration makes use of them.

::: tabs
== systemd
- **Login managers**: You should use the `fht-compositor` option
- **From a TTY**: Run `fht-compositor-session` from your TTY

== non-systemd
```sh
# While you can run without D-Bus, many things like the ScreenCast portal will not work!
dbus-run-session fht-compositor --session
```

:::

On startup, the compositor will try to generate a default configuration file inside `~/.config/fht/compositor.toml`.
You should use it as a base to build your configuration, with this Wiki.

Some important key-binds to know are:

| Binding | Keyaction |
| ------- | --------- |
| <kbd>Super+Enter</kbd> | Spawn alacritty |
| <kbd>Super+Shift+C</kbd> | Close focused window |
| <kbd>Super+P</kbd> | Spawn wofi |
| <kbd>Super+J/K</kbd> | Focus the next/previous window |
| <kbd>Super+Shift+J/K</kbd> | Swap current window with the next/previous window |
| <kbd>Super+[1-9]</kbd> | Focus the nth workspace |
| <kbd>Super+Shift+[1-9]</kbd> | Send the focused window to the nth workspace |
| <kbd>Super+Ctrl+Space</kbd> | Toggle floating on focused window |
| <kbd>Super+F</kbd> | Toggle fullscreen on focused window |
| <kbd>Super+M</kbd> | Toggle maximized on focused window |

Start to open some windows and get a feel for the compositor. You will immediatly notice the [dynamic layouts](/usage/layouts)
arranging the opened windows in a very special way: In a master and slave stack. The default layout, the `master/tile` tile,
arranges windows as follows:

![Master slave stack](/assets/master-slave-stacks.png)
