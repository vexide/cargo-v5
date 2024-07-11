{ rustPlatform, pkgs, pros-cli, ... }:

rustPlatform.buildRustPackage {
  pname = "cargo-pros";
  version = "0.6.0";

  src = ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
    outputHashes = {
      "vex_v5_serial-0.0.1" =
        "sha256-YK5k4uUmHkpFPhBxQuPp4tr2PrPFVi6imKUqbXP8t+0=";
    };
  };

  buildInputs = with pkgs; [ pkg-config openssl libclang dbus udev ];
}
