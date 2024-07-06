with import <nixpkgs> {};

pkgs.mkShell {
    buildInputs = with pkgs; [
        pkg-config
        openssl
        libclang
        dbus
        udev
    ];

    LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
}
