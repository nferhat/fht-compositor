# Configuration

`fht-compositor` it should read its configuration from the following paths in this order of precedence:

1. The [cli](../cli.md) argument `--config-path`
2. `$XDG_CONFIG_HOME/fht/compositor.toml`
3. `$HOME/.config/fht/compositor.toml`

If there's no configuration file present, the compositor will try to generate a default configuration file.

<details>
<summary>Default configuration contents</summary>

```toml
{{#include ../../../res/compositor.toml}}
```
</details>

## Configuration overview

The configuration is written in the [TOML](https://toml.io/en/) format, for its simplicity and readability.

> **NOTE**: Before the `0.1.2` release, the configuration file format was 
> [`ron`](https://github.com/ron-rs/ron), since `0.1.2`, the configuration
> switched to TOML, and there's sadly no automatic converter between the two
> (notably because they are structured very differently)

Specifying a key that is not supported by the configuration is considered an error.

The configuration is live-reloaded, the config file itself aswell as any imports are watched for modifications in order to reload the configuration.
> NOTE: For keybindings, the <kbd>Super</kbd> key is the <kbd>Windows</kbd>/<kbd>Logo</kbd> key under the `udev` backend, otherwise under the `X11` it becomes the <kbd>Alt</kbd> key.

When running the compositor for the first time, press <kbd>Super+Return</kbd> to start [Alacritty](https://github.com/alacritty), press <kbd>Super+P</kbd> to execute [wofi](https://hg.sr.ht/~scoopta/wofi), finally press <kbd>Super+Q</kbd> to quit the compositor.

Switching workspaces is done using <kbd>Super+[0-9]</kbd>, and you can send windows to another one using <kbd>Super+Shift+[0-9]</kbd>.

After the first startup, please refer to the [recommended software](./recommended-software.md) list in order to get everything up and running properly in your Wayland session.

You can also check out the [configuration reference](../configuration/index.md) for a list of all the configuration options you can modify
