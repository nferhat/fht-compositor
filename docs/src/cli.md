# Command-line interface

```
A dynamic tiling Wayland compositor

Usage: fht-compositor [OPTIONS] [COMMAND]

Commands:
  check-configuration   Check the compositor configuration for any errors
  generate-completions  Generate shell completions for shell
  help                  Print this message or the help of the given subcommand(s)

Options:
  -b, --backend <BACKEND>
          What backend should the compositor start with?

          Possible values:
          - udev: Use the Udev backend, using a libseat session
          - winit: Use the Winit backend, inside an Winit window


  -c, --config-path <PATH>
          The configuration path to use

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```
