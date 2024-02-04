{ rustPlatform, pkgs, ... }:

rustPlatform.buildRustPackage {
  pname = "cargo-pros";
  version = "0.4.0";

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  buildInputs = with pkgs; [
    pkg-config
    openssl
    gcc-arm-embedded-9
    clang
    libclang
    glibc_multi
  ];
}
