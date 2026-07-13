{ pkgs, ... }:

{
  packages = [
    pkgs.sqlite
    pkgs.cargo-llvm-cov
    pkgs.cargo-watch
    pkgs.pkgsCross.musl64.stdenv.cc
  ];

  env.CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
  env.CC_x86_64_unknown_linux_musl = "x86_64-unknown-linux-musl-gcc";

  dotenv.enable = true;

  languages.rust = {
    enable = true;
    channel = "stable";
    targets = [ "x86_64-unknown-linux-musl" ];
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" "llvm-tools-preview" ];
  };

  treefmt = {
    enable = true;
    config.programs = {
      nixpkgs-fmt.enable = true;
      rustfmt.enable = true;
    };
  };

  git-hooks.hooks = {
    treefmt.enable = true;
    clippy.enable = true;
    make-check = {
      enable = true;
      name = "make check";
      entry = "make check";
      language = "system";
      pass_filenames = false;
    };
  };
}
