{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, numtide, ... }: (numtide.lib.eachDefaultSystem
    (system:
      let pkgs = nixpkgs.legacyPackages.${system};
      devShell.${system} = import ./shell.nix;
    )
  );
}
