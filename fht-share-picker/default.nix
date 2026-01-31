{
  lib,
  glib,
  gtk4,
  libadwaita,
  libxkbcommon,
  # Yes, we also include the dependencies from fht-compositor, since we are building the cargo workspace
  # then specifically asking for fht-share-picker only. This shouldn't be a huge issue since you wouldn't
  # install fht-share-picker alone anyway.
  udev,
  seatd,
  libgbm,
  pipewire,
  dbus,
  libdisplay-info,
  libinput,
  pkg-config,
  rustPlatform,
  rev ? "unknown",
}:
rustPlatform.buildRustPackage {
  pname = "fht-share-picker";
  version = rev;
  src = ../.;

  cargoLock = {
    # The Cargo.lock file we use has some git deps
    allowBuiltinFetchGit = true;
    lockFile = ../Cargo.lock;
  };
  cargoBuildFlags = "--package fht-share-picker";

  strictDeps = true;
  # NOTE: We need glib in nativeBuildInputs for glib-compile-resources
  nativeBuildInputs = [rustPlatform.bindgenHook pkg-config glib];
  buildInputs = [glib gtk4 libadwaita libxkbcommon udev seatd dbus libgbm pipewire libdisplay-info libinput];

  meta = {
    homepage = "https://github.com/nferht/fht-share-picker";
    license = lib.licenses.gpl3Only;
    mainProgram = "fht-share-picker";
    platforms = lib.platforms.linux;
  };
}
