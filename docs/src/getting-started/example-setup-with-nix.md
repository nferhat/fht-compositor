# Example setup with Nix

The following page will describe to you how you can setup a good Wayland desktop session with Nix
flakes, NixOS, and home-manager. You can obviously adapt the following to suit your needs.

The setup includes `fht-compositor` itself, PipeWire for sound and screencast portal, GDM, and
shows you how to create user services that startup with the session using home-manager.

1. Add the [flake](../nix/flake.md) to your inputs.

2. NixOS part: You want to include the provided module `inputs.fht-compositor.nixosModules.default`
  and enable the following:

```nix
{ inputs, ... }:

{
  imports = [inputs.fht-compositor.nixosModules.default];

  programs = {
    dconf.enable = true; # required for dbus and GTK theming
    # Enabling the compositor here will ensure that the .desktop files are correctly created
    # and a UWSM session is instanciated. You'll find it with the name 'fht-compositor (UWSM)'
    fht-compositor = { enable = true; withUWSM = true; };
  };

  services = {
    # You probably want sound, right?
    # This is also required for the screencast portal.
    pipewire = {
      enable = true;
      alsa = { enable = true; support32Bit = true; };
      jack.enable = true;
      pulse.enable = true;
    };

    # Enable whatever display/session manager you like.
    # Or do without one, and run `uwsm start fht-compositor-uwsm.desktop` from a TTY.
    xserver.displayManager.gdm.enable = true;
  };

  # The compositor itself will start its own portal.
  # Otherwise enable GTK portal as a fallback.
  xdg.portal = {
    enable = true;
    xdgOpenUsePortal = true;
    config.common.default = ["gtk"];
    extraPortals = [pkgs.xdg-desktop-portal-gtk];
  };
}
```

3. The home-manager part: Configure the compositor with Nix and setup services. The following
  examples are from my [dotfiles](https://github.com/nferhat/dotfiles)

```nix
{ config, inputs, ... }:

{
  imports = [inputs.fht-compositor.homeModules.default];

  # Enable configuration.
  # NOTE: The final configuration is checked before being applied!
  programs.fht-compositor = {
    enable = true;
    settings = {
      # Include cursor configuration from home environment
      cursor = {inherit (config.home.pointerCursor) name size;};

      # I mean, its really up to you...
      # You can also just do `builtins.fromTOML` if you have an existing config

    };
  };

  # Services that we setup as part of the desktop/graphical session.
  # They get all triggered when fht-compositor reaches the graphical.target
  # ---
  # You are **REALLY** recommended to use systemd services/units for your
  # autostart instead of configuring them with the autostart section, since you also get restart
  # on failure, logs, and all nice stuff.
  systemd.user.services = let
    start-with-graphical-session = Description: {
      Unit = {
        inherit Description;
        After = ["graphical-session.target"];
        PartOf = ["graphical-session.target"];
        BindsTo = ["graphical-session.target"];
        Requisite = ["graphical-session.target"];
      };
      Install.WantedBy = ["graphical-session.target" "fht-compositor.service"];
    };
  in {
    wallpaper =
      start-with-graphical-session "Wallpaper service"
      // {
        Service = {
          Type = "simple";
          ExecStart = "${pkgs.swaybg}/bin/swaybg -i ${/path/to/wallpaper-file}";
          Restart = "on-failure";
        };
      };

    # For my personally, I like having xwayland satellite to play games.
    # It works really fine, I already play non-native stuff fine. Though for other programs it may
    # not work as good, for example windows that need positionning
    xwayland-sattelite =
      start-with-graphical-session "Xwayland-satellite"
      // {
        Service = {
          Type = "notify";
          NotifyAccess = "all";
          ExecStart = "${pkgs.xwayland-satellite}/bin/xwayland-satellite";
          StandardOutput = "jounral";
        };
      };
  };
}
```

4. Enjoy? Rebuild your system configuration and have fun I guess.
