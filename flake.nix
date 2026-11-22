{
  description = "muzik: download, split, and organize music from Soulseek, YouTube, and Bandcamp";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;

      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      muzikPackages =
        pkgs:
        let
          build = program: pkgs.callPackage ./nix/package.nix { inherit program; };
        in
        {
          muzik = build "cli";
          muzik-server = build "server";
        }
        // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          muzik-gpui = build "gui";
        };
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          muzik = muzikPackages pkgs;
        in
        muzik // { default = muzik.muzik; }
      );

      overlays.default = final: _prev: muzikPackages final;

      checks = forAllSystems (pkgs: self.packages.${pkgs.stdenv.hostPlatform.system});

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
