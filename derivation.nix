{ rustPlatform, pkgs, ... }:

rustPlatform.buildRustPackage {
  pname = "cargo-pros";
  version = "0.0.3";

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  buildInputs = with pkgs; [
    pkgconfig
    openssl
    gcc-arm-embedded-9
    clang
    libclang
    glibc_multi
  ];
}
