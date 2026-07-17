{
  description = "Wadachi (轍) — native typed-Rust directory frecency primitive for the pleme-io fleet (a zoxide replacement: frost smart-cd + mado overlay + shared SQLite store)";

  # substrate.rust.workspace dispatches over Cargo.gen.lock (the slim gen delta,
  # reconstructed to the full BuildSpec in pure Nix) — no crate2nix, no Cargo.nix.
  inputs.substrate.url = "github:pleme-io/substrate";

  outputs = { substrate, ... }: substrate.rust.workspace {
    src = ./.;
    member = "pleme-io-wadachi";
  };
}
