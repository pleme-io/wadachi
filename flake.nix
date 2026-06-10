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
  outputs = { self, nixpkgs, crate2nix, flake-utils, substrate, ... }:
    (import "${substrate}/lib/rust-workspace-release-flake.nix" {
      inherit nixpkgs crate2nix flake-utils;
    }) {
      toolName = "wadachi";
      packageName = "wadachi";
      src = self;
      repo = "pleme-io/wadachi";
    };
}
