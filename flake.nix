{
  description = "A dynamic tiling Wayland compositor.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    gitignore = {
      url = "github:hercules-ci/gitignore.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs:
    inputs.flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux"]; # TODO: aarch64? though I don't use it.
      perSystem = {
        self',
        pkgs,
        inputs',
        ...
      }: let
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
            version = self'.shortrev or self'.dirtyShortRev or "unknown";
            src = inputs.gitignore.lib.gitignoreSource ./.;

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

            postInstall = ''
              install -Dm644 res/fht-compositor.portal $out/share/xdg-desktop-portal
            '';

            env.RUSTFLAGS = toString (
              map (arg: "-C link-arg=" + arg) [
                "-Wl,--push-state,--no-as-needed"
                "-lEGL"
                "-lwayland-client"
                "-Wl,--pop-state"
              ]
            );

            meta = {
              description = "A dynamic tiling Wayland compositor.";
              homepage = "https://github.com/nferhat/fht-compositor";
              license = lib.licenses.gpl3Only;
              mainProgram = "fht-compositor";
              platforms = lib.platforms.linux;
            };
          };
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

        # TODO: NixOS module with configuration?
        # This should be simple since Nix has primitives to directly convert a nix table to TOML
        # 
        # One thing that would be neat is to check the configuration using the used fht-compositor
        # binary, since we support `fht-compositor --check-configuration ...`
      };
    };
}
