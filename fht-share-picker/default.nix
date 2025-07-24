{
  lib,
  glib,
  gtk4,
  libadwaita,
  libxkbcommon,
  pkg-config,
  rustPlatform,
  rev ? "unknown",
}:
rustPlatform.buildRustPackage {
  pname = "fht-share-picker";
  version = rev;
  src = ./.;

  cargoLock = {
    # The Cargo.lock file we use has some git deps
    allowBuiltinFetchGit = true;
    lockFile = ../Cargo.lock;
  };
  strictDeps = true;
  # NOTE: We need glib in nativeBuildInputs for glib-compile-resources
  nativeBuildInputs = [rustPlatform.bindgenHook pkg-config glib];
  buildInputs = [glib gtk4 libadwaita];

  meta = {
    homepage = "https://github.com/nferht/fht-compositor";
    license = lib.licenses.gpl3Only;
    mainProgram = "fht-share-picker";
    platforms = lib.platforms.linux;
  };
}
