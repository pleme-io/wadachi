# wadachi (轍) — directory frecency primitive

> **★★★ CSE / Knowable Construction.** Operates under Constructive Substrate
> Engineering; the Compounding Directive (solve once, load-bearing fixes,
> idiom-first, models stay current) is in the org-level pleme-io/CLAUDE.md.

`wadachi` owns *the directory you frequent* as a first-class fleet primitive:
record-on-cd → SQLite store (the shared inter-process bus) → ranked by the
shared `wadachi-spec` frecency core → consumed by frost (smart-cd), skim-cd
(ranked picker), and mado (OSC-7 feed + native overlay picker + MCP). It
**replaces** zoxide (import-only migration; never a runtime dep).

## Build & test

```bash
cargo test            # 8 tests: frecency matrix + store + doc-tests
# On bare macOS the link needs nix's libiconv on LIBRARY_PATH (chrono→CoreFoundation→iconv):
#   export LIBRARY_PATH=$(echo /nix/store/*libiconv*/lib | tr ' ' ':')
```

## Architecture

- `wadachi-spec/` — **zero-I/O** frecency core, the TYPED-SPEC + INTERPRETER
  TRIPLET. `apply(spec, entries, env)` is the one ranking formula; skim-tab
  adopts it (deleting its private `frecency_score`). `MockEnvironment` freezes
  the clock so tests are deterministic. `tests/frecency_matrix.rs` fails the
  build if a `DecayKind` lands without a row.
- `wadachi/` — `DirStore` trait (`DirFrecencyDb` SQLite + `MemDirStore` test
  seam) + `query::{top_n,top_match}` + `WadachiConfig` + `wadachi` CLI.

Store schema: `visits` (append-only real-visit log) + a **separate**
`discovered` table (indexer upserts; floored to `indexed_epsilon` so an indexed
dir is rankable but can't pollute real-visit frequency).

## Fast-follows (named destinations, deliberately staged interims)

The destination is fully specified; these are on the shortest path, staged so
the felt result (frost smart-cd + mado overlay) shipped first:

1. **`#[derive(DeriveTataraDomain)]` on `FrecencyRankingSpec`** — makes
   `(deffrecency-ranking …)` a first-class tatara-lisp keyword. Deferred because
   it pulls the full tatara-lisp closure as a git dep. The Rust constructors
   (`skimtab_parity()`/`zoxide_parity()`) currently mirror `specs/frecency.lisp`
   field-for-field; wiring the derive makes the Lisp the single source.
2. **`WadachiConfig : shikumi::TieredConfig`** — replace the plain config struct
   with the full tiered surface (bare/discovered/prescribed_default/extend/diff
   + `WADACHI_TIER` + `wadachi config-show <tier>`) per the configuration prime
   directive. The 9-field tier table is specified in the design doc.
3. **nix flake + auto-release** — `rust-workspace-release-flake.nix` + the
   3-line workspace auto-release shim + `Cargo.gen.lock`. (frost/mado consume
   wadachi as a git source, which doesn't need wadachi's own flake; this is for
   wadachi's standalone build/publish to crates.io.) v0.1.0 was seed-published
   manually pending the gen@HEAD auto-release regen fix.
4. **`ashiato-niwa` aggressive background indexer (M4)** — shigoto DAG of
   `WalkRootJob`s + `RootMtimeGate` + `notify` watcher walking `~/code/*/*`, so
   never-visited dirs become rankable jump targets (the GOAL's harder half).
5. **`import-zoxide`** — one-shot reader of `~/.local/share/zoxide/db.zo`.

## Consumers (build order)

`wadachi` (M0, done) → frost record-on-cd (M1) → smart-cd + ranked skim-cd +
frostmourne flip (M2) → mado native overlay (M3) → indexer (M4) → MCP+polish
(M5). Extension points: frost `frost-exec/src/env.rs:666` (chdir PWD chokepoint),
mado `src/terminal.rs:1536` (OSC-7) + `render.rs:2929` (snow-overlay template
for the picker), frost-lisp `integration.rs:154` (KNOWN_INTEGRATIONS).

skip-catalog: fewer than three authored Lisp domains.
