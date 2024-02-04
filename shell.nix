with import <nixpkgs> {};

pkgs.mkShell {
    buildInputs = with pkgs; [
        pkg-config
        openssl
        gcc-arm-embedded-9
        clang
        libclang
        glibc_multi
    ];

    LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
}
