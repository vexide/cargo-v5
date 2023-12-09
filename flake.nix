{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    nixpkgs,
    flake-utils,
    ...
  }:
    (flake-utils.lib.eachDefaultSystem
      (system:
        let 
          pkgs = nixpkgs.legacyPackages.${system};
        in rec {
          devShells.${system} = import ./shell.nix;

          packages = rec {
            cargo-pros = pkgs.callPackage ./derivation.nix {};
            default = cargo-pros;
          };

          apps = rec {
            cargo-pros = flake-utils.lib.mkApp { drv = packages.cargo-pros; };
            default = cargo-pros;
          };
        }
      )
    );
}
