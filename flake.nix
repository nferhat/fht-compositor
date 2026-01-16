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

  outputs = inputs:
    inputs.flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux"];
      imports = [./nix/packages.nix ./nix/hm-module.nix ./nix/nixos-module.nix];

      perSystem = {
        self',
        pkgs,
        ...
      }: {
        formatter = pkgs.alejandra;

        devShells.default = let
          rust-bin = inputs.rust-overlay.lib.mkRustBin {} pkgs;
          inherit (self'.packages) fht-compositor fht-share-picker;
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

            buildInputs = fht-compositor.buildInputs ++ fht-share-picker.buildInputs;
            nativeBuildInputs = fht-compositor.nativeBuildInputs ++ fht-share-picker.nativeBuildInputs;

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
    };
}
