with import <nixpkgs> {};

pkgs.mkShell {
    buildInputs = with pkgs; [
        pkgconfig
        openssl
        gcc-arm-embedded-9
        clang
        libclang
        glibc_multi
    ];

    LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
}