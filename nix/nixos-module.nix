{
  self,
  inputs,
  ...
}: {
  flake.nixosModules = rec {
    default = fht-compositor;
    fht-compositor = {
      lib,
      config,
      options,
      pkgs,
      ...
    }: let
      inherit (pkgs.stdenv.hostPlatform) system;
      inherit (self.packages.${system}) fht-compositor fht-share-picker;

      cfg = config.programs.fht-compositor;

      # wayland-session.nix setups some basic stuff that is technically optional but really good
      # to have in a Wayland session. All major compositor include it with their modules.
      wayland-session = import (inputs.nixpkgs + "/nixos/modules/programs/wayland/wayland-session.nix");
    in {
      options.programs.fht-compositor = {
        enable = lib.mkEnableOption "fht-compositor";
        package = lib.mkOption {
          type = lib.types.package;
          default = fht-compositor;
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
            # NOTE: the wayland-session.nix included in nixpkgs provides us with GTK and dconf
            environment.systemPackages = [fht-share-picker];
            xdg.portal.configPackages = [cfg.package];
          })

          (wayland-session {
            inherit lib pkgs;
            enableXWayland = false; # we dont have xwayland support
            enableWlrPortal = false; # fht-compositor ships its own portal.
          })
        ]
      );
    };
  };
}
