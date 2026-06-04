{
  description = "PaaSTech development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        pack = pkgs.stdenv.mkDerivation {
          pname = "pack";
          version = "0.40.6";
          src = pkgs.fetchurl {
            url = "https://github.com/buildpacks/pack/releases/download/v0.40.6/pack-v0.40.6-linux.tgz";
            hash = "sha256-SfuHT3qTBlODTmfBaRc2n5Q4CARAGUpkGEIbFxFCECg=";
          };
          phases = [ "installPhase" ];
          installPhase = ''
            mkdir -p $out/bin
            tar -xzf $src -C $out/bin pack
            chmod +x $out/bin/pack
          '';
        };
      in {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            sqlx-cli
            pack
            openssl
            pkg-config
          ];

          env.RUST_BACKTRACE = "1";
        };
      }
    );
}
