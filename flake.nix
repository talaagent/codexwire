{
  description = "codexwire: Rust client and protocol types for OpenAI Codex";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      perSystem = { system, pkgs, lib, self', ... }:
        let
          # Toolchain pinned by rust-toolchain.toml, from rust-overlay.
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rustPlatform = pkgs.makeRustPlatform { cargo = rustToolchain; rustc = rustToolchain; };
        in {
        _module.args.pkgs = import inputs.nixpkgs { inherit system; overlays = [ (import inputs.rust-overlay) ]; };
        packages.default = rustPlatform.buildRustPackage {
          pname = "codexwire";
          version = (lib.importTOML ./Cargo.toml).workspace.package.version;
          src = lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--workspace" "--all-features" ];
          useNextest = true;
          cargoTestFlags = [ "--workspace" "--all-features" ];
          nativeCheckInputs = [ pkgs.cacert pkgs.python3 ];
          postPatch = ''
            patchShebangs crates/*/tests
          '';
          # Nextest runs binaries; Rustdoc examples need their own check.
          postCheck = ''
            cargo test --workspace --all-features --doc --offline --release
          '';
          installPhase = ''
            runHook preInstall
            mkdir -p $out
            runHook postInstall
          '';
          meta.license = lib.licenses.asl20;
        };
        checks.default = self'.packages.default;
        devShells.default = pkgs.mkShell {
          packages = [ rustToolchain ] ++ (with pkgs; [ cargo-nextest cargo-deny python3 ]);
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
        };
        formatter = pkgs.nixfmt;
      };
    };
}
