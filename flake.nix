{
  description = "codexwire: Rust client and protocol types for OpenAI Codex";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      perSystem = { pkgs, lib, self', ... }: {
        packages.default = pkgs.rustPlatform.buildRustPackage {
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
          packages = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer cargo-nextest cargo-deny python3 ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
        formatter = pkgs.nixfmt;
      };
    };
}
