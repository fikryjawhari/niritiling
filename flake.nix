{
  description = "niritiling - automatic window tiling for the first window in Niri";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks-nix = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs: inputs.flake-parts.lib.mkFlake { inherit inputs; } {
    imports = [
      inputs.treefmt-nix.flakeModule
      inputs.git-hooks-nix.flakeModule
    ];

    systems = [ "x86_64-linux" "aarch64-linux" ];

    perSystem = { config, self', pkgs, ... }: {
      treefmt = {
        programs = {
          nixpkgs-fmt.enable = true;
          rustfmt.enable = true;
          deadnix.enable = true;
          statix.enable = true;
        };
      };

      pre-commit.settings = {
        settings = {
          rust = {
            cargoManifestPath = "./Cargo.toml";
            check.cargoDeps = pkgs.rustPlatform.importCargoLock {
              lockFile = ./Cargo.lock;
              outputHashes = {
                "niri-ipc-26.4.0" = "sha256-S7TyDDhu+PAoDSKxBHJ4BCTvg7Tu2ct1ZRZH6gOnN34=";
              };
            };
          };
        };
        hooks = {
          treefmt.enable = true;
          clippy.enable = true;
        };
      };

      packages = rec {
        niritiling = pkgs.rustPlatform.buildRustPackage {
          pname = "niritiling";
          version = "0.1.1";
          src = ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "niri-ipc-26.4.0" = "sha256-S7TyDDhu+PAoDSKxBHJ4BCTvg7Tu2ct1ZRZH6gOnN34=";
            };
          };
          meta.mainProgram = "niritiling";
        };
        default = niritiling;
      };

      devShells.default = pkgs.mkShell {
        inputsFrom = [ self'.packages.default ];
        nativeBuildInputs = with pkgs; [
          rust-analyzer
          clippy
        ];
        shellHook = config.pre-commit.installationScript;
      };
    };

    flake =
      let
        inherit (inputs.nixpkgs) lib;
      in
      {
        nixosModules = rec {
          niritiling = { pkgs, ... }: {
            imports = [ ./nix/module.nix ];

            options.services.niritiling.package = lib.mkOption {
              type = lib.types.package;
              description = "The niritiling package to use.";
              default = inputs.self.packages.${pkgs.stdenv.hostPlatform.system}.niritiling;
            };
          };
          default = niritiling;
        };
      };
  };
}
