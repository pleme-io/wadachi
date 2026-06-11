{
  description = "Wadachi (轍) — native typed-Rust directory frecency primitive for the pleme-io fleet (a zoxide replacement: frost smart-cd + mado overlay + shared SQLite store)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    crate2nix.url = "github:nix-community/crate2nix";
    flake-utils.url = "github:numtide/flake-utils";
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  # substrate's rust-workspace-release builder wraps crate2nix + eachSystem +
  # overlays so we don't hand-maintain cargoLock hashes or per-target
  # buildRustPackage invocations. `packageName = "wadachi"` selects the CLI
  # member; `wadachi-spec` is built as its workspace dependency. Same contract
  # as pleme-io/frost.
  #
  # `module` engages substrate's module-trio macro: one spec emits the
  # homeManagerModules / nixosModules / darwinModules outputs. The HM surface
  # is the codesearch/zoekt-mcp "daemon maintains the local db that powers
  # search" shape:
  #
  #   services.wadachi.enable         — install the CLI (the shared store bus)
  #   services.wadachi.indexer.enable — run `wadachi indexd` (ashiato-niwa) as
  #                                     a launchd agent (Darwin) / systemd user
  #                                     unit (Linux); alias that drives
  #                                     services.wadachi.daemon.enable.
  outputs = { self, nixpkgs, crate2nix, flake-utils, substrate, ... }:
    (import "${substrate}/lib/rust-workspace-release-flake.nix" {
      inherit nixpkgs crate2nix flake-utils;
    }) {
      toolName = "wadachi";
      # crate2nix selects the member by its Cargo package name.
      packageName = "pleme-io-wadachi";
      src = self;
      repo = "pleme-io/wadachi";
      module = {
        description = "wadachi (轍) directory frecency — shared store CLI + ashiato-niwa background indexer";
        hmNamespace = "services";
        withUserDaemon = true;
        userDaemonSubcommand = "indexd";
        extraHmOptions = lib: {
          indexer = {
            enable = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = ''
                Run the ashiato-niwa background directory indexer
                (`wadachi indexd`) as a user-level launchd agent (Darwin) /
                systemd user unit (Linux). Sets
                services.wadachi.daemon.enable by default — but the
                module-trio gates ALL config (this alias included) on
                services.wadachi.enable, so BOTH switches are required:
                  services.wadachi.enable = true;
                  services.wadachi.indexer.enable = true;
                Use services.wadachi.daemon.{extraArgs,environment} for
                knobs (e.g. WADACHI_TIER / WADACHI_DB).
              '';
            };
          };
        };
        extraHmConfigFn = { cfg, lib, ... }: {
          services.wadachi.daemon.enable = lib.mkDefault (cfg.indexer.enable or false);
        };
      };
    };
}
