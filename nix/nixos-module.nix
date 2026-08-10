{
  config,
  lib,
  options,
  pkgs,
  ...
}: let
  cfg = config.programs.fht-compositor;

  # wayland-session.nix setups some basic stuff that is technically optional but really good
  # to have in a Wayland session. All major compositor include it with their modules.
  wayland-session = lib.mkMerge (
    [
      {
        security = {
          polkit.enable = true;
          pam.services.swaylock = {};
        };
        programs.dconf.enable = lib.mkDefault true;
        services.xserver.desktopManager.runXdgAutostartIfNone = lib.mkDefault true;
      }
    ]
    ++ lib.optional (options.services ? graphical-desktop) {
      services.graphical-desktop.enable = true;
      xdg.portal.extraPortals = [pkgs.xdg-desktop-portal-gtk];
    }
    ++ lib.optional (!(options.services ? graphical-desktop)) {
      fonts.enableDefaultPackages = lib.mkDefault true;
      programs.xwayland.enable = lib.mkDefault false;
    }
  );
in {
  options.programs.fht-compositor = {
    enable = lib.mkEnableOption "fht-compositor";
    package = lib.mkOption {
      type = lib.types.package;
      description = "The fht-compositor package to use.";
      default = pkgs.callPackage ../default.nix {};
    };
    # Package used to select what to screencast, installed with the xdg-screencast-portal.
    sharePickerPackage = lib.mkOption {
      type = lib.types.package;
      description = "The fht-share-picker package to use with the screencast portal.";
      default = pkgs.callPackage ../fht-share-picker/default.nix {};
      internal = true;
    };
  };

  # Module config copied from hyprland.nix in official nixpkgs.
  # We also include additional recommended software to ease the experience
  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        environment.systemPackages = [cfg.package pkgs.xdg-utils];

        # OpenGL/mesa is required. We do not have a software renderer.
        hardware =
          if lib.strings.versionAtLeast config.system.nixos.release "24.11"
          then {
            graphics.enable = lib.mkDefault true;
          }
          else {
            opengl.enable = lib.mkDefault true;
          };

        services.gnome.gnome-keyring.enable = true;
        systemd.user.services.fht-compositor-polkit = {
          description = "PolicyKit Authentication Agent provided by fht-compositor";
          wantedBy = ["fht-compositor.service"];
          after = ["graphical-session.target"];
          partOf = ["graphical-session.target"];
          serviceConfig = {
            Type = "simple";
            ExecStart = "${pkgs.polkit_gnome}/libexec/polkit-gnome-authentication-agent-1";
            Restart = "on-failure";
            RestartSec = 1;
            TimeoutStopSec = 10;
          };
        };
      }

      {
        # Install the fht-compositor package to display servers in order to make the .desktop
        # file discoverable (providing a fht-compositor desktop entry)
        services =
          if lib.strings.versionAtLeast config.system.nixos.release "24.05"
          then {
            displayManager.sessionPackages = [cfg.package];
          }
          else {
            xserver.displayManager.sessionPackages = [cfg.package];
          };
      }

      (lib.mkIf (builtins.elem "xdg-screencast-portal" cfg.package.buildFeatures) {
        # Install the share-picker application in order to select what to screencast.
        # NOTE: the wayland-session setup provides us with GTK and dconf
        environment.systemPackages = [cfg.sharePickerPackage];
        xdg.portal.configPackages = [cfg.package];
      })

      wayland-session
    ]
  );
}
