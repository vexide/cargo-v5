{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    numtide,
    flake-utils,
    ...
  }: 
    (flake-utils.lib.eachDefaultSystem
      (system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in with pkgs; rec {
          devShells.${system} = import ./shell.nix;
        }
      )
    );
}
