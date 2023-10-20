{ rustPlatform, fetchgit, lib, pkgs, ... }:

rustPlatform.buildRustPackage {
  pname = "cargo-pros";
  version = "0.0.3";

  src = ./.;

  cargoHash = "sha256-llkdJZ7PhsLzHYgy7AblMIGtBX2FwJquHYGZLs3bV6g=";

  buildInputs = with pkgs; [
    pkgconfig
    openssl
    gcc-arm-embedded-9
    clang
    libclang
    glibc_multi
  ];
}
