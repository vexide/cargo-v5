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
              extensions = [ "rust-analyzer" "rust-src" "clippy" ];
            })
          ];

          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        };

        packages = let
          naersk' = pkgs.callPackage naersk {
            cargo = pkgs.rust-bin.stable.latest.default;
            rustc = pkgs.rust-bin.stable.latest.default;
          };
          build-cargo-v5 = { features ? [ ], ... }:
            pkgs.lib.checkListOfEnum "cargo-v5: features" [
              "field-control"
              "fetch-template"
            ] features naersk'.buildPackage {
              name = "cargo-v5";
              pname = "cargo-v5";
              version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;

              src = ./.;

              passthru = {
                withFeatures = features: build-cargo-v5 { inherit features; };
              };

              cargoBuildOptions = opts:
                opts ++ [
                  "--features"
                  ''"${builtins.concatStringsSep " " features}"''
                ];

              nativeBuildInputs = with pkgs; [ pkg-config dbus udev ];
            };
        in rec {
          cargo-v5 = build-cargo-v5 { };
          default = cargo-v5;
        };

        apps = rec {
          cargo-v5 = flake-utils.lib.mkApp { drv = packages.cargo-v5; };
          default = cargo-v5;
        };
      }));
}
