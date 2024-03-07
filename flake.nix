{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    pros-cli-nix.url = "github:BattleCh1cken/pros-cli-nix";
  };

  outputs = {
    nixpkgs,
    flake-utils,
    pros-cli-nix,
    ...
  }:
    (flake-utils.lib.eachDefaultSystem
      (system:
        let 
          pkgs = nixpkgs.legacyPackages.${system};
        in rec {
          devShells.${system} = import ./shell.nix;

          packages = rec {
            cargo-pros = pkgs.callPackage ./derivation.nix { pros-cli = pros-cli-nix.packages.${system}.default; };
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
