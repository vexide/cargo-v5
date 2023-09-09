with import <nixpkgs> {};

pkgs.mkShell {
    buildInputs = with pkgs; [
        pkgconfig
        openssl
    ];
};