{
  description = "Static Rust Discord RSS Bot";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      packages.${system}.default = pkgs.pkgsStatic.callPackage ./default.nix { };
    };
}
