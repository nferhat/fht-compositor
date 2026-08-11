{
  description = "A dynamic tiling Wayland compositor.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    ...
  }: let
    rev = self.shortRev or self.dirtyShortRev or "unknown";
    pkgs = nixpkgs.legacyPackages.x86_64-linux;
  in {
    formatter.x86_64-linux = pkgs.alejandra;

    packages.x86_64-linux = import ./nix/packages.nix {inherit pkgs rev;};

    overlays.x86-64-linux.default = import ./nix/overlay.nix {inherit rev;};

    nixosModules = {
      fht-compositor = import ./nix/nixos-module.nix;
      default = self.nixosModules.fht-compositor;
    };

    homeModules = {
      fht-compositor = import ./nix/hm-module.nix;
      default = self.homeModules.fht-compositor;
    };

    devShell.x86_64-linux = let
      rust-bin = rust-overlay.lib.mkRustBin {} pkgs;
      inherit (self.packages.x86_64-linux) fht-compositor fht-share-picker;
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
          pkgs.tracy # profiler
          pkgs.alejandra # for formatting this flake if needed
          pkgs.prettier # formatting documentation
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
}
