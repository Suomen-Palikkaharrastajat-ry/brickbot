{ lib
, rustPlatform
,
}:

rustPlatform.buildRustPackage {
  pname = "brickbot";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = lib.cleanSource ./.;
    filter = path: type:
      let
        baseName = baseNameOf (toString path);
      in
      baseName == "Cargo.toml" ||
      baseName == "Cargo.lock" ||
      baseName == "migrations" ||
      baseName == "locales" ||
      baseName == ".sqlx" ||
      lib.hasSuffix ".rs" baseName ||
      lib.hasSuffix ".sql" baseName ||
      lib.hasSuffix ".yml" baseName ||
      type == "directory";
  };

  cargoLock.lockFile = ./Cargo.lock;
  # CARGO_BUILD_TARGET is automatically handled by pkgsStatic
}
