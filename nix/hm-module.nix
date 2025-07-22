{self', ...}: {
  flake.homeModules = rec {
    default = fht-compositor;
    # NOTE: This module implementation is directly ripped from home-manager's helix module
    #   home-manager/modules/programs/helix.nix
    fht-compositor = {
      lib,
      config,
      options,
      pkgs,
      ...
    }: let
      inherit (self'.packages) fht-compositor;
      cfg = config.programs.fht-compositor;
      tomlFormat = pkgs.formats.toml {};

      # Custom config format that also runs checks on the final config file.
      configFormat = {
        inherit (tomlFormat) type;
        generate = name: value: let
          # First we generate the result with tomlFormat.
          result = tomlFormat.generate name value;
          # Then we evaluate the result
          checkResult = pkgs.runCommand "fht-compositor-check-configuration" {} ''
            ${cfg.package}/bin/fht-compositor --config-path ${result} check-configuration
            ln -s ${result} $out
          '';
        in
          checkResult;
      };
    in {
      options.programs.fht-compositor = {
        enable = lib.mkEnableOption "fht-compositor";
        package = lib.mkOption {
          type = lib.types.package;
          default = fht-compositor;
        };

        settings = lib.mkOption {
          type = configFormat.type;
          default = {};
          example = lib.literalExpression ''
            {
              autostart = [];
              general.cursor-warps = true;

              decorations.border = {
                thickness = 3;
                radius = 0;
                focused-color = {
                  start = "#5781b9";
                  end = "7fc8db";
                  angle = 0;
                };
              };

              animations.disable = false;

              keybinds."Super-q" = "quit";

              rules = [
                { on-workspace = 5; floating = true; centered = true };
                # other window rules...
              ]
            }
          '';
          description = ''
            Configuration written to
            {file}`$XDG_CONFIG_HOME/fht/compositor.toml`.
          '';
        };
      };

      config = lib.mkIf cfg.enable {
        home.packages = [cfg.package];
        xdg.configFile.fht-compositor-config = lib.mkIf (cfg.settings != {}) {
          target = "fht/compositor.toml";
          source = configFormat.generate "fht-compositor-config" cfg.settings;
          onChange = ''
            (
              export FHTC_SOCKET_PATH=$(find /run/user/$(id -u) -type s -name 'fhtc-*-wayland-*.socket' -print -quit)
              ${lib.getExe cfg.package} ipc action reload-config
            )
          '';
        };
      };
    };
  };
}
