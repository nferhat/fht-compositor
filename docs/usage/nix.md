# Nix modules

To ease setups with [NixOS](https://nixos.org) and [home-manager](https://github.com/nix-community/home-manager/), the
[`fht-compositor` repository](https://github.com/nferhat/fht-compositor) provides NixOS and home-manager modules. While it
is also a [Nix Flake](https://nixos.wiki/wiki/flakes), the modules and packages are plain Nix expressions and work
**without** flakes as well.

## Using flakes
You can add it to your configuration as follows:

```nix
{
  inputs = {
    # Currently only tested against unstable, but in theory should work fine with latest
    # stable release. If anything goes wrong, report an issue!
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    fht-compositor = {
      url = "github:nferhat/fht-compositor";
      inputs.nixpkgs.follows = "nixpkgs";

      # Disable rust-overlay since it's only meant to be here for the devShell provided
      # (IE. only for development purposes, end users don't care)
      inputs.rust-overlay.follows = "";
    };
  }
}
```

## Using without flakes
Everything also works from a plain git checkout, so you don't need flakes enabled in your configuration:

```nix
{ pkgs, ... }:

let
  fht-compositor = import /path/to/fht-compositor; # or use a channel / fetchTarball
in {
  imports = [
    "${fht-compositor}/nix/nixos-module.nix"
    # For home-manager:
    # "${fht-compositor}/nix/hm-module.nix"
  ];

  programs.fht-compositor.enable = true;

  # Optionally, make the packages available as `pkgs.fht-compositor` and
  # `pkgs.fht-share-picker` through an overlay:
  nixpkgs.overlays = [ (import "${fht-compositor}/nix/overlay.nix" {}) ];
}
```

Alternatively, you can build the packages directly with `pkgs.callPackage` or use the overlay on their own:

```nix
pkgs.fht-compositor = pkgs.callPackage /path/to/fht-compositor/default.nix {};
pkgs.fht-share-picker = pkgs.callPackage /path/to/fht-compositor/fht-share-picker/default.nix {};
```

> [!NOTE] Package source
> When using the modules without the flake, the `programs.fht-compositor.package` option defaults to a build from source
> using your system's nixpkgs. With flakes, you can pin it to the flake's own build with:
> `programs.fht-compositor.package = inputs.fht-compositor.packages.${pkgs.system}.fht-compositor;`

## NixOS module

This module lets you enable `fht-compositor` and expose it to display managers like GDM, and enable required configuration
(mesa, hardware acceleration, etc.) It also setups nice-to-have features for a fuller desktop session:

- A [polkit agent](https://wiki.archlinux.org/title/Polkit#Authentication_agents): `polkit-gnome` to be exact
- [GNOME keyring](https://wiki.gnome.org/Projects/GnomeKeyring): Authentification agent
- [xdg-desktop-portal-gtk](https://github.com/flatpak/xdg-desktop-portal-gtk): Fallback portal

To enable it, include it `inputs.fht-compositor.nixosModules.default`

---

#### `programs.fht-compositor.enable`

Whether to enable `fht-compositor`

---

#### `programs.fht-compositor.package`

The `fht-compositor` package to use.

Default: a build from source using your system's nixpkgs (`pkgs.callPackage ../default.nix {}`)

## home-manager module

This module lets you easily configure `fht-compositor` through home-manager module system.

To enable it, include it `inputs.fht-compositor.homeModules.default`

---

#### `programs.fht-compositor.enable`

Whether to enable `fht-compositor`

---

#### `programs.fht-compositor.package`

The `fht-compositor` package to use.

Default: a build from source using your system's nixpkgs (`pkgs.callPackage ../default.nix {}`)

---

#### `programs.fht-compositor.settings`

Configuration table written directly to `$XDG_CONFIG_HOME/fht/compositor.toml`. Since Nix and TOML have a one-to-one mapping, all
the data types and structures you have in TOML can be easily re-written in Nix.

> [!TIP] Configuration check
> `programs.fht-compositor.settings` is checked against `programs.fht-compositor.package`! Using the following command line
> ```sh
> fht-compositor check-configuration /path/to/generated/compositor.toml
> ```
> If your configuration have any issues, home-manager **will not** rebuild your configuration!

A possible alternative is to use `builtins.fromTOML`:

```nix
{
  programs.fht-compositor.settings = builtins.fromTOML ./path/to/compositor.toml;
}
```
