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
      pipewire,
      dbus,
      wayland,
      pkg-config,
      rustPlatform,
      # Optional stuff that can be toggled on by the user.
      # These correspond to cargo features.
      withUdevBackend ? true,
      withWinitBackend ? true,
      withXdgScreenCast ? true,
      withProfiling ? false,
    }:
      rustPlatform.buildRustPackage {
        pname = "fht-compositor";
        version = self.shortRev or self.dirtyShortRev or "unknown";
        src = ./.;

        postPatch = ''
          patchShebangs res/fht-compositor-session
          substituteInPlace res/fht-compositor.service \
            --replace-fail '/usr/bin' "$out/bin"
        '';

        cargoLock = {
          # NOTE: Since dependencies such as smithay are only distributed with git,
          # we are forced to allow cargo to fetch them.
          allowBuiltinFetchGit = true;
          lockFile = ./Cargo.lock;
        };

        strictDeps = true;

        nativeBuildInputs = [rustPlatform.bindgenHook pkg-config];
        buildInputs =
          [libGL libdisplay-info libinput seatd libxkbcommon mesa wayland]
          ++ lib.optional withXdgScreenCast dbus
          ++ lib.optional withXdgScreenCast pipewire;

        # NOTE: Whenever adding features, don't forget to specify them here!!
        buildFeatures =
          lib.optional withXdgScreenCast "xdg-screencast-portal"
          ++ lib.optional withWinitBackend "winit-backend"
          ++ lib.optional withUdevBackend "udev-backend"
          ++ lib.optional withProfiling "profile-with-puffin";
        buildNoDefaultFeatures = true;

        postInstall =
          ''
            install -Dm644 res/fht-compositor.desktop -t $out/share/wayland-sessions
            # Supporting session targets. Maybe add a systemd option?
            install -Dm755 res/fht-compositor-session $out/bin/fht-compositor-session
            install -Dm644 res/fht-compositor{.service,-shutdown.target} -t $out/share/systemd/user
          ''
          + lib.optionalString withXdgScreenCast ''
            install -Dm644 res/fht-compositor.portal -t $out/share/xdg-desktop-portal
          '';

        env.RUSTFLAGS = toString (
          map (arg: "-C link-arg=" + arg) [
            "-Wl,--push-state,--no-as-needed"
            "-lEGL"
            "-lwayland-client"
            "-Wl,--pop-state"
          ]
        );

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
      systems = ["x86_64-linux"]; # TODO: aarch64? though I don't use it.
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
          pkgs.mkShell {
            packages = [
              # For developement purposes, a nightly toolchain is preferred.
              # We use nightly cargo for formatting, though compiling is limited to
              # whatever is specified inside ./rust-toolchain.toml
              (rust-bin.selectLatestNightlyWith (toolchain:
                toolchain.default.override {
                  extensions = ["rust-analyzer" "rust-src"];
                }))
              pkgs.alejandra # for formatting this flake if needed
              pkgs.nodePackages.prettier # formatting documentation
            ];

            inherit (fht-compositor) buildInputs nativeBuildInputs;
            env = {
              # WARN: Do not overwrite this variable in your shell!
              # It is required for `dlopen()` to work on some libraries; see the comment
              # in the package expression
              #
              # This should only be set with `CARGO_BUILD_RUSTFLAGS="$CARGO_BUILD_RUSTFLAGS -C your-flags"`
              CARGO_BUILD_RUSTFLAGS = fht-compositor.RUSTFLAGS;
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
        in {
          options.programs.fht-compositor = {
            enable = lib.mkEnableOption "fht-compositor";
            package = lib.mkOption {
              type = lib.types.package;
              default = pkgs.callPackage fht-compositor-package { };
            };
          };

          config = lib.mkMerge [
            {
              # Require an XDG environment. Makes our life 100x easier.
              environment.systemPackages = [pkgs.xdg-utils];
              xdg = {
                autostart.enable = lib.mkDefault true;
                menus.enable = lib.mkDefault true;
                mime.enable = lib.mkDefault true;
                icons.enable = lib.mkDefault true;
              };
            }

            (lib.mkIf cfg.enable {
              # Install the fht-compositor package to display servers in order to make the .desktop
              # file discoverable (providing a fht-compositor desktop entry)
              services =
                if lib.strings.versionAtLeast config.system.nixos.release "24.05"
                then {
                  displayManager.sessionPackages = [cfg.package];
                }
                else
                {
                  xserver.displayManager.sessionPackages = [cfg.package];
                };
              # OpenGL/mesa is required. We do not have a software renderer.
              hardware =
                if lib.strings.versionAtLeast config.system.nixos.release "24.11"
                then {
                  graphics.enable = lib.mkDefault true;
                }
                else {
                  opengl.enable = lib.mkDefault true;
                };
            })

            (lib.mkIf cfg.enable {
              environment.systemPackages = [cfg.package];
              services.gnome.gnome-keyring.enable = true;
              # Provide the xdg-desktop-portal-gtk portal for users, since we only cover the screencast
              # one with the compositor. Fallback on GTK for everything else.
              xdg.portal = {
                enable = true;
                extraPortals = lib.mkIf (
                  !cfg.package.cargoBuildNoDefaultFeatures || builtins.elem "xdg-screencast-portal" cfg.package.cargoBuildFeatures
                ) [pkgs.xdg-desktop-portal-gtk];
                configPackages = [cfg.package];
              };

              # These also contribute to making our life 100x easier, as well as providing a more
              # fleshed out setup out of the box.
              security.polkit.enable = true;
              programs.dconf.enable = lib.mkDefault true;
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
            })
          ];
        };
      };
    };
}
