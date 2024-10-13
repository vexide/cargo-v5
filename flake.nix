{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nix-community/naersk";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, flake-utils, naersk, rust-overlay, ... }:
    (flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
      in rec {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            pkg-config
            openssl
            libclang
            dbus
            udev
            (pkgs.rust-bin.nightly.latest.default.override {
              extensions = [ "rust-analyzer" "rust-src" "clippy" "llvm-tools" ];
            })
          ];

          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        };

        packages = rec {
          cargo-v5 = let
            naersk' = pkgs.callPackage naersk {
              cargo = pkgs.rust-bin.stable.latest.default;
              rustc = pkgs.rust-bin.stable.latest.default;
            };
          in naersk'.buildPackage {
            name = "cargo-v5";
            pname = "cargo-v5";
            version = "0.8.1";

            src = ./.;

            nativeBuildInputs = with pkgs; [ pkg-config dbus udev ];
          };

          default = cargo-v5;
        };

        apps = rec {
          cargo-v5 = flake-utils.lib.mkApp { drv = packages.cargo-v5; };
          default = cargo-v5;
        };
      }));
}
