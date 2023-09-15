{ rustPlatform, fetchgit, lib, pkgs, ... }:

rustPlatform.buildRustPackage {
  pname = "cargo-pros";
  version = "0.0.3";

  src = ./.;

  cargoHash = "sha256-BjU3GrCdKjjuTAk8OJUi+BEgB2VckmxnRg+HN2oL9WI=";

  buildInputs = with pkgs; [
    pkgconfig
    openssl
    gcc-arm-embedded-9
    clang
    libclang
    glibc_multi
  ];
}
