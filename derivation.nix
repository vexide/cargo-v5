{ rustPlatform, fetchgit, lib, pkgs, ... }:

rustPlatform.buildRustPackage {
  pname = "cargo-pros";
  version = "0.0.1";

  src = ./.;

  cargoHash = "sha256-c3zbb/0hCAgT0A/EjeFl5HfNZ4QNbMA/jyNInn7/N7A=";

  nativeBuildInputs = with pkgs; [ pkgconfig openssl ];
  buildInputs = with pkgs; [ ];
}
