{
  description = "Sideporch: a small, self-hosted team chat in one binary";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
      forSystem = system:
        let
          pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
          manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          # The same toolchain as rust-toolchain.toml and mise.toml.
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          platform = pkgs.makeRustPlatform { cargo = toolchain; rustc = toolchain; };
          package = platform.buildRustPackage {
            pname = "sideporch";
            version = manifest.package.version;
            src = pkgs.lib.fileset.toSource {
              root = ./.;
              fileset = pkgs.lib.fileset.unions [
                ./Cargo.toml ./Cargo.lock ./rust-toolchain.toml ./clippy.toml ./build.rs
                ./encre-css.toml ./src ./assets ./tests ./README.md ./LICENSE
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;
            # The end-to-end tests start real servers on 127.0.0.1.
            __darwinAllowLocalNetworking = true;
            doCheck = true;
            doInstallCheck = true;
            installCheckPhase = ''
              runHook preInstallCheck
              "$out/bin/sideporch" --version
              runHook postInstallCheck
            '';
            meta = {
              description = manifest.package.description;
              homepage = manifest.package.repository;
              license = pkgs.lib.licenses.mit;
              mainProgram = "sideporch";
              platforms = systems;
            };
          };
        in { inherit pkgs package toolchain; };
    in {
      packages = eachSystem (system: { default = (forSystem system).package; });
      apps = eachSystem (system: {
        default = { type = "app"; program = "${self.packages.${system}.default}/bin/sideporch"; };
      });
      checks = eachSystem (system: { package = self.packages.${system}.default; });
      devShells = eachSystem (system:
        let env = forSystem system;
        in { default = env.pkgs.mkShell { packages = [ env.toolchain env.pkgs.zig env.pkgs.cargo-zigbuild ]; }; });
      nixosModules.default = { config, lib, pkgs, ... }:
        let cfg = config.services.sideporch;
        in {
          options.services.sideporch = {
            enable = lib.mkEnableOption "Sideporch, a small self-hosted team chat";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              description = "The Sideporch package to run.";
            };
            listen = lib.mkOption {
              type = lib.types.str;
              default = "127.0.0.1:8080";
              description = "Address and port to listen on.";
            };
            publicUrl = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "https://chat.example.com";
              description = "The URL people use to reach Sideporch.";
            };
          };
          config = lib.mkIf cfg.enable {
            systemd.services.sideporch = {
              description = "Sideporch team chat";
              wantedBy = [ "multi-user.target" ];
              after = [ "network-online.target" ];
              wants = [ "network-online.target" ];
              environment = {
                SIDEPORCH_LISTEN = cfg.listen;
                SIDEPORCH_DATA = "/var/lib/sideporch";
              } // lib.optionalAttrs (cfg.publicUrl != null) { SIDEPORCH_PUBLIC_URL = cfg.publicUrl; };
              serviceConfig = {
                ExecStart = lib.getExe cfg.package;
                DynamicUser = true;
                StateDirectory = "sideporch";
                Restart = "on-failure";
                NoNewPrivileges = true;
                ProtectSystem = "strict";
                ProtectHome = true;
                PrivateTmp = true;
              };
            };
          };
        };
    };
}
