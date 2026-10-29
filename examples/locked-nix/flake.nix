{
  description = "Prepared Python workers for the Metewand locked example";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/c7def046b9a883d46974757852106483d741586f";
  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      eachSystem = f: builtins.listToAttrs (map (system: { name = system; value = f system; }) systems);
    in {
      packages = eachSystem (system: { python = nixpkgs.legacyPackages.${system}.python3; });
      apps = eachSystem (system: {
        python3 = { type = "app"; program = "${nixpkgs.legacyPackages.${system}.python3}/bin/python3"; };
      });
    };
}
