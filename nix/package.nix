# Build one muzik program from the Cargo workspace.
#
# `program` selects the Cargo package:
#   "cli"    -> `muzik`, the command-line program
#   "gui"    -> `muzik-gpui`, the desktop app (Linux only; macOS uses the Homebrew cask)
#   "server" -> `muzik-server`, the web app
{
  lib,
  stdenv,
  rustPlatform,
  makeBinaryWrapper,
  pkg-config,
  sqlite,
  ffmpeg,
  yt-dlp,
  fontconfig,
  freetype,
  libxkbcommon,
  wayland,
  libxcb,
  libx11,
  vulkan-loader,
  libGL,
  program ? "cli",
}:

let
  workspace = lib.importTOML ../Cargo.toml;

  programs = {
    cli = {
      pname = "muzik";
      package = "muzik-cli";
      mainProgram = "muzik";
      description = "Download, split, and organize music from Soulseek, YouTube, and Bandcamp";
    };
    gui = {
      pname = "muzik-gpui";
      package = "muzik-gpui";
      mainProgram = "muzik-gpui";
      description = "Desktop app to download, split, and organize music from Soulseek, YouTube, and Bandcamp";
    };
    server = {
      pname = "muzik-server";
      package = "muzik-server";
      mainProgram = "muzik-server";
      description = "Web app to download, split, and organize music from Soulseek, YouTube, and Bandcamp";
    };
  };

  selected = programs.${program};
  isGui = program == "gui";

  # Only the files Cargo reads. Docs, packaging, and CI changes do not rebuild.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../apps
      ../crates
      ../assets/muzik-logo-64.png
    ];
  };

  # The programs muzik starts at run time. They follow the user's PATH, so a
  # newer yt-dlp that the user installed wins over this one.
  runtimePath = lib.makeBinPath [
    ffmpeg
    yt-dlp
  ];

  # GPUI opens these with dlopen, so they must be on the rpath.
  guiRuntimeLibraries = [
    vulkan-loader
    wayland
    libxkbcommon
    libGL
  ];
in
rustPlatform.buildRustPackage {
  inherit (selected) pname;
  inherit (workspace.workspace.package) version;
  inherit src;

  cargoLock = {
    lockFile = ../Cargo.lock;
    # Git dependencies from `[patch.crates-io]` in Cargo.toml. When a `rev`
    # changes there, set its hash to `lib.fakeHash` and copy the new one from
    # the `nix build` error.
    outputHashes = {
      "bolero-generator-derive-0.13.5" = "sha256-RLGeqVZL6H9KYN9rlOOZqojIb2Pj5aZiFmpSjTdBPNU=";
    };
  };

  cargoBuildFlags = [
    "--package"
    selected.package
  ];

  nativeBuildInputs = [
    makeBinaryWrapper
    pkg-config
  ];

  # The nixpkgs cargo hook sets LIBSQLITE3_SYS_USE_PKG_CONFIG, so rusqlite
  # links this SQLite instead of the copy that its `bundled` feature builds.
  buildInputs = [
    sqlite
  ]
  ++ lib.optionals isGui [
    fontconfig
    freetype
    libxkbcommon
    wayland
    libxcb
    libx11
  ];

  # CI runs the full test suite with `mise run check`. Many tests start
  # ffmpeg, yt-dlp, and other programs, or need the network.
  doCheck = false;

  postFixup =
    lib.optionalString isGui ''
      patchelf $out/bin/muzik-gpui --add-rpath ${lib.makeLibraryPath guiRuntimeLibraries}
    ''
    + ''
      wrapProgram $out/bin/${selected.mainProgram} --suffix PATH : ${runtimePath}
    '';

  meta = {
    inherit (selected) description mainProgram;
    homepage = "https://github.com/TudorAndrei/muzik";
    changelog = "https://github.com/TudorAndrei/muzik/blob/main/CHANGELOG.md";
    license = lib.licenses.gpl3Only;
    platforms = if isGui then lib.platforms.linux else lib.platforms.unix;
  };
}
