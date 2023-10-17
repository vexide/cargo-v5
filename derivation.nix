{ rustPlatform, fetchgit, lib, pkgs, ... }:

rustPlatform.buildRustPackage {
  pname = "cargo-pros";
  version = "0.0.3";

  src = ./.;

  cargoHash = "sha256-XLZa1MmdyzIViMhrKiycXE4uVKqxij5EWcXeVeC+P0o=";

  buildInputs = with pkgs; [
    pkgconfig
    openssl
    gcc-arm-embedded-9
    clang
    libclang
    glibc_multi
  ];
}
