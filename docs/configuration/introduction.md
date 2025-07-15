# Configuration in `fht-compositor`

`fht-compositor` is configured using a [TOML](https://toml.io), a very simple and obvious key-value language. The configuration
contents itself is broken down into multiple sub-sections:

- [Top-level section](#top-level-section)
- [General behaviour](./general)
- [Input configuration](./input)
- [Key-bindings](./keybindings)
- [Mouse-bindings](./mousebindings)
- [Window rules](./window-rules)
- [Layer-shell rules](./layer-rules)
- [Outputs](./outputs)
- [Cursor theme](./cursor)
- [Decorations](./decorations)
- [Animations](./animations)

## Loading

The compositor will try to load your configuration from the following paths, with decreasing order of precedence:

1. `--config-path`/`-c` command line argument.
2. `$XDG_CONFIG_HOME/fht/compositor.toml`
3. `~/.config/fht/compositor.toml`

If there's no configuration in second/third paths, the compositor will generate a
[template configuration file](https://github.com/nferhat/fht-compositor/blob/main/res/compositor.toml). You should
use it as a base for further modifications.

## Configuration reloading

The configuration is live-reloaded. You can edit and save the file and `fht-compositor` will automatically pick up and
apply changes.

If you made a mistake when writing your configuration (let that be syntax, invalid values, unknown enum variant, etc.), the
compositor will warn you with a popup window sliding from the top of your screen. You can run `fht-comopsitor check-configuration`
to get that error in your terminal.


## Top-level section

##### `autostart`

Command lines to run whenever the compositor starts up. Each line is evaluated using `/bin/sh -c "<command line>"`, meaning you have
access to shell-expansions like using variables, or exapding of `~` to `$HOME`

> [!TIP] Autostart with systemd units
> While using this approach for autostart can work, using systemd user services are a much better! You benefit from having
> logs using `journalctl -xeu --user {user-service}`, restart-on-failure, and bind them to specific targets.

> [!NOTE] XDG autostart on non-systemd distros
> Most desktop programs under Linux that have a "Run on computer startup" or programs that make use of XDG autostart will *not*
> work by default! You will need a program like [dex](https://github.com/jceb/dex) for it.
>
> ```toml
> autostart = ["dex -a"] # autostart XDG desktop entries/programs
> ```
>
> A notable example is PipeWire that makes use of this to startup with the desktop session.

---

##### `env`

Environment variables to set. They are set before anything else starts up in the compositor, and before `autostart` is exxecuted

```toml
[env]
DISPLAY = ":727"
_JAVA_AWT_NONREPARENTING = "1"
```
