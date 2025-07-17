{
  description = "A dynamic tiling Wayland compositor.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    fht-share-picker = {
      url = "github:nferhat/fht-share-picker/gtk-rewrite";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-parts.follows = "flake-parts";
      inputs.rust-overlay.follows = "";
    };
  };

  outputs = inputs @ {self, ...}: let
    # NOTE: For now function lives with the outputs declaration since its used by perSystem.packages
    # and also flake.nixosModules.fht-compositor. A better solution (that I haven't figured out)
    # is use the overlay provided by this flake in the nixos module
    fht-compositor-package = {
      lib,
      libGL,
      libdisplay-info,
      libinput,
      seatd,
      libxkbcommon,
      mesa,
      libgbm,
      pipewire,
      dbus,
      wayland,
      pkg-config,
      rustPlatform,
      installShellFiles,
      # Optional stuff that can be toggled on by the user.
      # These correspond to cargo features.
      withUdevBackend ? true,
      withWinitBackend ? true,
      withXdgScreenCast ? true,
      withSystemd ? true,
      withProfiling ? false,
    }:
      rustPlatform.buildRustPackage {
        pname = "fht-compositor";
        version = self.shortRev or self.dirtyShortRev or "unknown";
        src = ./.;

        preFixup = ''
          mkdir completions

          for shell in bash fish zsh ; do
            $out/bin/fht-compositor generate-completions $shell > completions/fht-compositor.$shell
          done

          installShellCompletion completions/*
        '';

        postPatch = ''
          patchShebangs res/systemd/fht-compositor-session
          substituteInPlace res/systemd/fht-compositor.service \
            --replace-fail '/usr/bin' "$out/bin"
        '';

        cargoLock = {
          # NOTE: Since dependencies such as smithay are only distributed with git,
          # we are forced to allow cargo to fetch them.
          allowBuiltinFetchGit = true;
          lockFile = ./Cargo.lock;
        };

        strictDeps = true;

        nativeBuildInputs = [rustPlatform.bindgenHook pkg-config installShellFiles];
        buildInputs =
          [libGL libdisplay-info libinput seatd libxkbcommon mesa libgbm wayland]
          ++ lib.optional withXdgScreenCast dbus
          ++ lib.optional withXdgScreenCast pipewire;

        # NOTE: Whenever adding features, don't forget to specify them here!!
        buildFeatures =
          lib.optional withXdgScreenCast "xdg-screencast-portal"
          ++ lib.optional withWinitBackend "winit-backend"
          ++ lib.optional withUdevBackend "udev-backend"
          ++ lib.optional withSystemd "systemd"
          ++ lib.optional withProfiling "profile-with-puffin";
        buildNoDefaultFeatures = true;

        postInstall =
          ''
            # Install .desktop file to be discoverable by Login managers
            install -Dm644 res/systemd/fht-compositor.desktop -t $out/share/wayland-sessions
            # And install systemd service files used by the .desktop file, including
            # the script that sets up the session
            install -Dm755 res/systemd/fht-compositor-session -t $out/bin/
            install -Dm644 res/systemd/fht-compositor.service -t $out/share/systemd/user
            install -Dm644 res/systemd/fht-compositor-shutdown.target -t $out/share/systemd/user
          ''
          + lib.optionalString withXdgScreenCast ''
            install -Dm644 res/fht-compositor.portal -t $out/share/xdg-desktop-portal/portals
            install -Dm644 res/fht-compositor-portals.conf -t $out/share/xdg-desktop-portal
          '';

        env = {
          RUSTFLAGS = toString (
            map (arg: "-C link-arg=" + arg) [
              "-Wl,--push-state,--no-as-needed"
              "-lEGL"
              "-lwayland-client"
              "-Wl,--pop-state"
            ]
          );
          # Make GIT_HASH available so that the fht-compositor -V reports it correctly.
          GIT_HASH = self.shortRev or self.dirtyShortRev or "unknown";
        };

        passthru.providedSessions = ["fht-compositor"];

        meta = {
          description = "A dynamic tiling Wayland compositor.";
          homepage = "https://github.com/nferhat/fht-compositor";
          license = lib.licenses.gpl3Only;
          mainProgram = "fht-compositor";
          platforms = lib.platforms.linux;
        };
      };
  in
    inputs.flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux"];
      perSystem = {
        self',
        pkgs,
        inputs',
        ...
      }: let
      in {
        # NOTE: This is for the Nix code formatter!!
        formatter = pkgs.alejandra;

        packages = rec {
          fht-compositor = pkgs.callPackage fht-compositor-package {};
          default = fht-compositor;
          # This build is only for dev purposes. it is meant to be build fast for
          # fast development. It is also the reason why its not stripped.
          #
          # Preferably if you are developing the compositor, you'd want to enter the provided
          # dev shell and run `cargo build ...`
          fht-compositor-debug = fht-compositor.overrideAttrs (next: prev: {
            pname = prev.pname + "-debug";
            cargoBuildType = "debug";
            cargoCheckType = next.cargoBuildType;
            dontStrip = true;
          });
        };

        devShells.default = let
          rust-bin = inputs.rust-overlay.lib.mkRustBin {} pkgs;
          inherit (self'.packages) fht-compositor;
        in
          pkgs.mkShell.override {
            stdenv = pkgs.stdenvAdapters.useMoldLinker pkgs.clangStdenv;
          } {
            packages = [
              # For developement purposes, a nightly toolchain is preferred.
              # We use nightly cargo for formatting, though compiling is limited to
              # whatever is specified inside ./rust-toolchain.toml
              (rust-bin.selectLatestNightlyWith (toolchain:
                toolchain.default.override {
                  extensions = ["rust-analyzer" "rust-src" "rustc-codegen-cranelift-preview"];
                }))
              pkgs.tracy-wayland # profiler
              pkgs.alejandra # for formatting this flake if needed
              pkgs.nodePackages.prettier # formatting documentation
              pkgs.nodejs # vitepress for docs
            ];

            inherit (fht-compositor) buildInputs nativeBuildInputs;
            env = {
              # WARN: Do not overwrite this variable in your shell!
              # It is required for `dlopen()` to work on some libraries; see the comment
              # in the package expression
              #
              # This should only be set with `CARGO_BUILD_RUSTFLAGS="$CARGO_BUILD_RUSTFLAGS -C your-flags"`
              CARGO_BUILD_RUSTFLAGS = "${fht-compositor.RUSTFLAGS} -Zcodegen-backend=cranelift";
            };
          };
      };

      flake.nixosModules = rec {
        default = fht-compositor;
        fht-compositor = {
          lib,
          config,
          options,
          pkgs,
          ...
        }: let
          cfg = config.programs.fht-compositor;
          fht-share-picker-pkg = inputs.fht-share-picker.packages."${pkgs.system}".default;
          wayland-session = import (inputs.nixpkgs + "/nixos/modules/programs/wayland/wayland-session.nix");
        in {
          options.programs.fht-compositor = {
            enable = lib.mkEnableOption "fht-compositor";
            package = lib.mkOption {
              type = lib.types.package;
              default = pkgs.callPackage fht-compositor-package { };
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
                environment.systemPackages = [fht-share-picker-pkg];
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
              default = pkgs.callPackage fht-compositor-package {};
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
    };
}
