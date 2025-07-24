{
  self,
  pkgs,
  ...
}: {
  perSystem = {pkgs, ...}: {
    packages = let
      rev = self.shortRev or self.dirtyShortRev or "unknown";
    in rec {
      fht-compositor = pkgs.callPackage ../default.nix {inherit rev;};
      default = fht-compositor;
      # This build is only for dev purposes to test the `nix build` output to see
      # that everything is correctly installed, otherwise, you should not be using
      # this.
      #
      # Preferably if you are developing the compositor, you'd want to enter the provided
      # dev shell and run `cargo build ...`
      fht-compositor-debug = fht-compositor.overrideAttrs (next: prev: {
        pname = prev.pname + "-debug";
        cargoBuildType = "debug";
        cargoCheckType = next.cargoBuildType;
        dontStrip = true;
      });

      # Companion program required for XDG screencast portal to work properly.
      fht-share-picker = pkgs.callPackage ../fht-share-picker/default.nix {inherit rev;};
    };
  };
}
