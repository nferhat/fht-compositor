# Command-line interface

Usage: `fht-compositor [OPTIONS] [COMMAND]`

## Commands

- `check-configuration`: Check the compositor configuration for any errors

## Options

- `-b`/`--backend`: What backend should the compositor start with?
  - `x11`: Use the X11 backend, inside an X11 window.
  - `udev`: Use the Udev backend, using a libseat session.

- `-c`/`--config-path`: The configuration path to use
