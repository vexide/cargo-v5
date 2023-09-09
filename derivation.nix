{ rustPlatform, fetchgit, lib, pkgs, ... }:

rustPlatform.buildRustPackage {
  pname = "cargo-pros";
  version = "0.0.1";

  src = ./.;

  cargoHash = "sha256-BsKkxFILlzPfSh9ko3msn4wQ0/4MSYydpgXNEIUnLUM=";

  buildInputs = with pkgs; [
    pkgconfig
    openssl
    gcc-arm-embedded-9
    clang
    libclang
    glibc_multi
  ];
}
