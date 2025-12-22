{
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
  # For versionning.
  rev ? "unknown",
  # Optional stuff that can be toggled on by the user.
  # These correspond to cargo features.
  withUdevBackend ? true,
  withWinitBackend ? true,
  withXdgScreenCast ? true,
  withXdgGlobalShortcuts ? true,
  withSystemd ? true,
  withProfiling ? false,
}:
rustPlatform.buildRustPackage {
  pname = "fht-compositor";
  version = rev;
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
    ++ lib.optional withXdgGlobalShortcuts "xdg-global-shortcuts-portal"
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
    GIT_HASH = rev;
  };

  passthru.providedSessions = ["fht-compositor"];

  meta = {
    description = "A dynamic tiling Wayland compositor.";
    homepage = "https://github.com/nferhat/fht-compositor";
    license = lib.licenses.gpl3Only;
    mainProgram = "fht-compositor";
    platforms = lib.platforms.linux;
  };
}
