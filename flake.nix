{
  description = "rldx - terminal PIM for vCard contacts";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/11cb3517b3af6af300dd6c055aeda73c9bf52c48";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        # Package build
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "rldx";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ openssl ]
            ++ lib.optionals stdenv.isDarwin [
              libiconv
              darwin.apple_sdk.frameworks.Security
            ];

          meta = {
            description = "Terminal PIM for vCard contacts";
            homepage = "https://github.com/verdigris12/rldx";
            license = pkgs.lib.licenses.gpl3Plus;
            mainProgram = "rldx";
          };
        };

        # Dev shell
        devShells.default = pkgs.mkShell {
          name = "rldx-dev";
          packages = with pkgs; [
            cargo rustc clippy rustfmt
            pkg-config sqlite
            openssl
            sccache
            git just
          ] ++ lib.optionals stdenv.isDarwin [
            libiconv
            darwin.apple_sdk.frameworks.Security
          ];

          shellHook = ''
            export RUST_BACKTRACE=1
            export RUSTC_WRAPPER=${pkgs.sccache}/bin/sccache
            echo "rldx dev-shell ready → cargo run -- --help"
          '';
        };

        # App that runs the built package
        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/rldx";
        };
      }
    );
}
