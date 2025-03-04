# Nix flake

The [GitHub repository](https://github.com/nferhat/fht-compositor) of the compositor is a Nix flake.
You can make use of it as follows

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fht-compositor = {
      url = "github:nferhat/fht-compositor";
      inputs.nixpkgs.follows = "nixpkgs";

      # If you are using flake-parts for your flake, override it to avoid duplication
      # inputs.flake-parts.follows = "flake-parts";

      # Disable rust-overlay from getting pulled on your flake.
      #
      # Why? Since its only used for the devShell provided with this flake. You
      # probably don't care about it
      inputs.rust-overlay.follows = "";
    };
  };
}
```

## NixOS module

The NixOS module installs `fht-compositor` system-wide and registers it with login managers.

```nix
{
  inputs,
  ...
}: {
  imports = [inputs.fht-compositor.nixosModules.default];

  programs.fht-compositor = {
    enable = true;

    # You most likely want to use UWSM to manage your wayland session.
    # It's a wrapper script in python that binds systemd targets and units to make stuff like
    # XDG autostart and dbus activation work.
    #
    # It is enabled by default, and you should not disable it.
    withUWSM = true;
  };
}
```

The NixOS module not only enables `fht-compositor`, but also enables you the following

- The GTK XDG desktop portal, we use this as a fallback for portal support
- `services.gnome.gnome-keyring`, recommended software
- `gnome-polkit-agent`, recommended software
- XDG autostart, used by "Run on startup" option in your applications

## [home-manager](https://github.com/nix-community/home-manager) module

Use this module if you need to configure `fht-compositor` declaratively using Nix.

```nix
{
  inputs,
  ...
}: {
  imports = [inputs.fht-compositor.homeModules.default];

  programs.fht-compositor = {
    enable = true;

    # Settings table.
    # The compositor itself uses toml for configuration, so its a one-to-one mapping between a
    # nix attrset and a TOML document. Automatically generates `~/.config/fht/compositor.toml`
    #
    # WARN: This configuration is automatically checked against
    # `config.programs.fht-compositor.package`, so if there's a config error, the home-manager
    # derivation will not build!
    settings = {};
  };
}
```
