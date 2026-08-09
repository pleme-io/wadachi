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
cargo test            # 34 tests: 18 frecency matrix + 6 indexer integration + 8 unit + 2 doc
cargo clippy --all-targets -- -D warnings   # clean (verified on clippy 1.96 stable)
# On bare macOS the link needs nix's libiconv on LIBRARY_PATH (chrono→CoreFoundation→iconv):
#   export LIBRARY_PATH=$(echo /nix/store/*libiconv*/lib | tr ' ' ':')
```

## Architecture

- `wadachi-spec/` — **zero-I/O** frecency core, the TYPED-SPEC + INTERPRETER
  TRIPLET. `apply(spec, entries, env)` is the one ranking formula; skim-tab
  adopts it (deleting its private `frecency_score`). `MockEnvironment` freezes
  the clock so tests are deterministic. `tests/frecency_matrix.rs` fails the
  build if a `DecayKind` or a `CombineKind` lands without a row.

  **How the score folds is `CombineKind`, not a fixed expression.** It was one
  hard-coded additive line in the interpreter until 2026-08-09, which made the
  additive shape the only expressible one — so a consumer needing a different
  one (tear's `praca`) had no move left but to re-implement the curve locally,
  which it did, byte-for-byte. `RecencySumPlusFreq` is the old behavior and the
  serde default; `FreqTimesLatestDecay` (`n × decay(last visit)`) is what zoxide
  and praça actually compute, shipped as the named instance `praca-parity`.
  `FrecencyRankingSpec::score_counted` is the entry point for a consumer that
  stores a **visit counter + one timestamp** instead of a per-visit log — it
  reaches the same `combine_score` the `Combine` phase does.
- `wadachi/` — `DirStore` trait (`DirFrecencyDb` SQLite + `MemDirStore` test
  seam) + `query::{top_n,top_match}` + `WadachiConfig` + `indexer` (M4, below)
  + `wadachi` CLI.

Store schema: `visits` (append-only real-visit log) + a **separate**
`discovered` table (indexer upserts; floored to `indexed_epsilon` so an indexed
dir is rankable but can't pollute real-visit frequency).

### ashiato-niwa — the M4 background indexer (`wadachi/src/indexer.rs`)

Walks the configured roots collecting **directories only**, upserts them into
`discovered` (never `visits`), prunes rows whose dir no longer exists, and in
daemon mode keeps the index live via `notify` + gate-checked re-walks:

- **Typed pieces**: `IgnoreSet` (never-descend names + hidden-dir gate) ·
  `walk_root` (iterative, never follows symlinks → cycles unrepresentable) ·
  `walk_roots` (bounded-parallel; pure FS — the single caller thread owns all
  store writes, so `DirStore: &self` needs no `Sync`) · `RootMtimeGate`
  (re-walk when root mtime shows direct-children churn OR the rewalk interval
  — the prune staleness bound — elapses) · `index_once` (walk+upsert+prune;
  `IndexSummary` renders via `Display`) · `apply_event_path` (the testable
  watcher event core: live admissible dir → upsert; gone path → subtree
  removal) · `run_daemon` (initial pass → recursive `notify` watch per root →
  debounced event flush, storm-bounded at 4096 → 30s gate checks).
- **Store seam**: `DirStore` grew `discovered_under(prefix)` +
  `remove_discovered(path)` (subtree removal; SQLite uses a byte-range match,
  not `LIKE`, so `%`/`_` in paths can't corrupt it). `MemDirStore` mirrors —
  every indexer integration test runs against **both** stores.
- **Canonical roots (load-bearing)**: roots are `fs::canonicalize`d at the
  engine border — macOS FSEvents reports resolved real paths
  (`/private/var/…`), so a symlinked root spelling would never match its own
  events. One spelling for walk/watch/event/gate ⇒ one `discovered` row per dir.
- **CLI**: `wadachi index` (one-shot pass) + `wadachi indexd` (daemon). Both
  honor `WadachiConfig::active()` (`WADACHI_TIER`/`WADACHI_DB` overlays);
  `indexer.enabled` gates the HM daemon, not explicit CLI invocations.
- **Config**: `WadachiConfig.indexer: IndexerConfig` group — `roots:
  Vec<IndexerRoot{path,max_depth}>`, `ignore_names`, `index_hidden`,
  `debounce_ms`, `rewalk_interval_secs`, `concurrency`. Prescribed defaults:
  roots `~/code` (depth 6) + `$HOME` (depth 2); ignore `.git node_modules
  target __pycache__ .direnv .cache result .cargo .rustup Library .Trash`;
  hidden dirs skipped; debounce 500ms; rewalk 900s. notify backend is
  target-gated like shikumi's (Linux inotify; macOS opts into FSEvents — NOT
  kqueue, which burns an fd per watched dir).

### Daemon wiring (HM module via substrate module-trio)

`flake.nix` passes `module = {…}` to `rust-workspace-release-flake.nix`, which
emits the `homeManagerModules`/`nixosModules`/`darwinModules` trio. The HM
surface is the codesearch/zoekt-mcp "daemon maintains the local db that powers
search" shape:

```nix
services.wadachi.enable = true;          # install the CLI (the shared store bus)
services.wadachi.indexer.enable = true;  # run `wadachi indexd` as a launchd agent
                                         # (Darwin) / systemd user unit (Linux)
```

`indexer.enable` drives `services.wadachi.daemon.enable` (mkDefault — still
overridable); `services.wadachi.daemon.{extraArgs,environment}` carry knobs
(e.g. `WADACHI_TIER`, `WADACHI_DB`). Agent label: `io.pleme.wadachi.daemon`.

### Named interims (per the interim-step rule)

- **Work graph**: `walk_roots` is a typed in-process work-queue
  (`Mutex<VecDeque<_>>` + scoped threads, bounded by `indexer.concurrency`) —
  the destination is a `shigoto` Dag of `WalkRootJob`s + `RootMtimeGate` as a
  shigoto Gate. Deferred because shigoto drags the tokio + gen-platform
  closure into every shell-linked consumer (frost links this crate in-process
  on the `cd` hot path); migrate when shigoto is consumable without that
  weight or behind a feature gate.
- **Debounce starvation bound**: a continuous event storm faster than the
  debounce window delays the gate-checked re-walk; flushes are forced at 4096
  pending events, so staleness stays bounded rather than typed away.

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
4. **`ashiato-niwa` aggressive background indexer (M4)** — **DONE** (see the
   indexer section above). Never-visited dirs are rankable jump targets (the
   GOAL's harder half). Residual: the shigoto-Dag migration is a named
   interim, and the `(deffrecency-ranking …)`-style Lisp authoring of
   `IndexerConfig` rides fast-follow #1/#2.
5. **`import-zoxide`** — one-shot reader of `~/.local/share/zoxide/db.zo`.

## Consumers (build order)

`wadachi` (M0, done) → frost record-on-cd (M1) → smart-cd + ranked skim-cd +
frostmourne flip (M2) → mado native overlay (M3) → indexer (M4, done) →
MCP+polish (M5). Extension points: frost `frost-exec/src/env.rs:666` (chdir PWD chokepoint),
mado `src/terminal.rs:1536` (OSC-7) + `render.rs:2929` (snow-overlay template
for the picker), frost-lisp `integration.rs:154` (KNOWN_INTEGRATIONS).

skip-catalog: fewer than three authored Lisp domains.
