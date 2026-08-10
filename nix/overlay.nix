{
  rev ? "unknown",
}: final: _: {
  fht-compositor = final.callPackage ../default.nix {
    inherit rev;
  };

  # Tool used with fht-compositor's screencast portal.
  fht-share-picker = final.callPackage ../fht-share-picker/default.nix {
    inherit rev;
  };
}
