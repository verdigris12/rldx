{
  description = "rldx dev environment";


  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/11cb3517b3af6af300dd6c055aeda73c9bf52c48";
    flake-utils.url = "github:numtide/flake-utils";
  };


  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
        rec {
        devShells.default = pkgs.mkShell {
          name = "rldx-dev";
          packages = with pkgs; [
            cargo rustc clippy rustfmt
            pkg-config sqlite
            openssl
            sccache
            git just
            codex
          ] ++ lib.optionals stdenv.isDarwin [ libiconv Security ];


          shellHook = ''
export RUST_BACKTRACE=1
export RUSTC_WRAPPER=${pkgs.sccache}/bin/sccache
echo "rldx dev-shell ready → cargo run --features dav -- --help"
          '';
        };


        apps.rldx = {
          type = "app";
          program = let script = pkgs.writeShellScriptBin "rldx" ''
set -euo pipefail
exec cargo run -- "$@"
          '';
          in "${script}/bin/rldx";
        };


        apps.default = apps.rldx;
      }
    );
}
