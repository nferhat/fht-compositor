# Configuration reference

`fht-compositor` it should read its configuration from the following paths in this order of precedence:

1. The [cli](../cli.md) argument `--config-path`
2. `$XDG_CONFIG_HOME/fht/compositor.toml`
3. `$HOME/.config/fht/compositor.toml`

You can run `fht-compositor check-configuration` in order to *check* you configuration for any errors using the TOML parser and deserializer.

Here is the configuration structure
- [Top-level section](#-top-level)
- [`general` section](./general.md)
- [`input` section](./input.md)
- [`cursor` section](./cursor.md)
- [`keybinds` section](./keybinds.md)

## Top-level section

- `imports`

Additional configuration files to import and merge with the main configuration file definitions. It skips over non-existing files and invalid configuration files.

Configuration file paths must be relative to system root `/` or the user home directory `~/`

Default is `[]`

------

 - `autostart`

Command lines to execute at the start of the compositor. The command lines are evaluated using `/bin/sh -c <command-line>`, so you benefit from shell expansion.

Default is `[]`
