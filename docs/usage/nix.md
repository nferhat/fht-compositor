# Nix modules

To ease setups with [NixOS](https://nixos.org) and [home-manager](https://github.com/nix-community/home-manager/), the
[`fht-compositor` repository](https://github.com/nferhat/fht-compositor) is a [Nix Flake](https://nixos.wiki/wiki/flakes).

You can add it to your configuration like the following.

```nix
{
  inputs = {
    # Currently only tested against unstable, but in theory should work fine with latest
    # stable release. If anything goes wrong, report an issue!
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    fht-compositor = {
      url = "github:nferhat/fht-compositor";
      inputs.nixpkgs.follows = "nixpkgs";

      # If you make use of flake-parts yourself, override here
      # inputs.flake-parts.follows = "flake-parts";

      # Disable rust-overlay since it's only meant to be here for the devShell provided
      # (IE. only for developement purposes, end users don't care)
      inputs.rust-overlay.follows = "";
    };
  }
}
```

## NixOS module

This module lets you enable `fht-compositor` and expose it to display managers like GDM, and enable required configuration
(mesa, hardware acceleration, etc.) It also setups nice-to-have features for a fuller desktop session:

- A [polkit agent](https://wiki.archlinux.org/title/Polkit#Authentication_agents): `polkit-gnome` to be exact
- [GNOME keyring](https://wiki.gnome.org/Projects/GnomeKeyring): Authentification agent
- [xdg-desktop-portal-gtk](https://github.com/flatpak/xdg-desktop-portal-gtk): Fallback portal
- [UWSM](https://github.com/Vladimir-csp/uwsm) session script.

To enable it, include it `inputs.fht-compositor.nixosModules.default`

---

#### `programs.fht-compositor.enable`

Whether to enable `fht-compositor`

---

#### `programs.fht-compositor.package`

The `fht-compositor` package to use.

Default: `<fht-compositor-flake>.packages.${pkgs.system}.fht-compositor`

---

#### `programs.fht-compositor.withUWSM`

Launch the fht-compositor session with UWSM (Universal Wayland Session Manager). Using this is highly recommended since it
improves fht-compositor's systemd support by binding appropriate targets like `graphical-session.target`,
`wayland-session@fht-compositor.target`, etc. for a regular desktop session.

## home-manager module

This module lets you easily configure `fht-compositor` through home-manager module system.

To enable it, include it `inputs.fht-compositor.homeModules.default`

---

#### `programs.fht-compositor.enable`

Whether to enable `fht-compositor`

---

#### `programs.fht-compositor.package`

The `fht-compositor` package to use.

Default: `<fht-compositor-flake>.packages.${pkgs.system}.fht-compositor`

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
