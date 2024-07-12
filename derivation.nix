{ naersk, pkgs, ... }:

naersk.buildPackage {
  name = "cargo-v5";
  pname = "cargo-v5";
  version = "0.6.1";

  src = ./.;

  nativeBuildInputs = with pkgs; [ pkg-config dbus udev ];
}
