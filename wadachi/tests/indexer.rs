//! M4 `ashiato-niwa` indexer integration tests — walk semantics, the
//! `discovered`-vs-`visits` separation, prune-on-rewalk, the
//! `indexed_epsilon` ranking floor, and the typed config surface. Each
//! store-facing test runs against BOTH `DirFrecencyDb` (real schema,
//! in-memory `SQLite`) and `MemDirStore` so the test seam can't drift.

use std::fs;
use std::path::Path;

use pleme_io_wadachi::config::{IndexerConfig, IndexerRoot, WadachiConfig};
use pleme_io_wadachi::indexer::{self, EventOutcome, IgnoreSet};
use pleme_io_wadachi::store::{DirFrecencyDb, DirStore, MemDirStore};
use pleme_io_wadachi::query;
use wadachi_spec::FrecencyRankingSpec;

fn ignore() -> IgnoreSet {
    IgnoreSet::new(["node_modules".to_owned(), "target".to_owned()], false)
}

fn cfg_for(root: &Path, max_depth: usize) -> IndexerConfig {
    IndexerConfig {
        enabled: true,
        roots: vec![IndexerRoot::new(root, max_depth)],
        ignore_names: vec!["node_modules".to_owned(), "target".to_owned()],
        index_hidden: false,
        debounce_ms: 50,
        rewalk_interval_secs: 60,
        concurrency: 4,
    }
}

/// Fixture:
/// ```text
/// root/
///   a/b/c/          (c is depth 3)
///   node_modules/x/ (ignored — never descended)
///   .hidden/y/      (hidden — skipped unless index_hidden)
///   file.txt        (not a dir)
/// ```
///
/// The returned root is canonicalized — the indexer canonicalizes roots at
/// its border (macOS tempdirs live under the `/var → /private/var` symlink),
/// so assertions must compare against the canonical spelling.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("a").join("b").join("c")).unwrap();
    fs::create_dir_all(root.join("node_modules").join("x")).unwrap();
    fs::create_dir_all(root.join(".hidden").join("y")).unwrap();
    fs::write(root.join("file.txt"), b"not a dir").unwrap();
    (tmp, root)
}

#[test]
fn walk_respects_ignore_set_and_max_depth() {
    let (_tmp, root) = fixture();
    let root = root.as_path();

    let mut found = indexer::walk_root(root, 2, &ignore());
    found.sort();
    assert_eq!(
        found,
        vec![root.join("a"), root.join("a").join("b")],
        "depth 2 walk must see a + a/b only — no depth-3 c, no ignored, no hidden, no files"
    );

    // index_hidden = true lets dotdirs in (but the named ignore set still holds).
    let permissive = IgnoreSet::new(["node_modules".to_owned()], true);
    let mut found = indexer::walk_root(root, 1, &permissive);
    found.sort();
    assert_eq!(found, vec![root.join(".hidden"), root.join("a")]);
}

fn assert_upserts_land_in_discovered(store: &impl DirStore) {
    let (_tmp, root) = fixture();
    let cfg = cfg_for(&root, 3);

    let summary = indexer::index_once(store, &cfg).unwrap();
    assert_eq!(summary.roots_walked, 1);
    assert_eq!(summary.indexed, 3, "a, a/b, a/b/c");
    assert_eq!(summary.pruned, 0);

    let entries = store.entries().unwrap();
    assert_eq!(entries.len(), 3);
    for e in &entries {
        assert!(e.discovered_only, "{} must be discovered, not visited", e.path.display());
        assert!(e.visits.is_empty(), "indexer must never write a visit row");
    }
}

#[test]
fn upserts_land_in_discovered_not_visits() {
    assert_upserts_land_in_discovered(&DirFrecencyDb::open_in_memory().unwrap());
    assert_upserts_land_in_discovered(&MemDirStore::new());
}

fn assert_rewalk_prunes_deleted(store: &impl DirStore) {
    let (_tmp, root) = fixture();
    let cfg = cfg_for(&root, 3);

    indexer::index_once(store, &cfg).unwrap();
    assert_eq!(store.entries().unwrap().len(), 3);

    // The whole `a` subtree disappears between walks.
    fs::remove_dir_all(root.join("a")).unwrap();
    let summary = indexer::index_once(store, &cfg).unwrap();
    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.pruned, 3, "a, a/b, a/b/c rows are all dead");
    assert!(store.entries().unwrap().is_empty());
}

#[test]
fn rewalk_prunes_deleted_dirs() {
    assert_rewalk_prunes_deleted(&DirFrecencyDb::open_in_memory().unwrap());
    assert_rewalk_prunes_deleted(&MemDirStore::new());
}

fn assert_indexed_ranks_below_real_visit(store: &impl DirStore) {
    let (_tmp, root) = fixture();
    let cfg = cfg_for(&root, 3);
    let spec = FrecencyRankingSpec::skimtab_parity();

    // One real visit (recorded "now" → score ≈ 1.0) + three indexed dirs.
    store.record("/real/visited").unwrap();
    indexer::index_once(store, &cfg).unwrap();

    let ranked = query::top_n(store, &spec, "", 50).unwrap();
    assert_eq!(ranked.len(), 4);
    assert_eq!(
        ranked[0].path,
        Path::new("/real/visited"),
        "a real visit must outrank every indexed-only dir"
    );
    for r in &ranked[1..] {
        assert!(
            (r.score - spec.indexed_epsilon).abs() < f64::EPSILON,
            "indexed-only {} must be floored to exactly indexed_epsilon, got {}",
            r.path.display(),
            r.score
        );
        assert!(r.score < ranked[0].score);
    }

    // And an indexed-but-never-visited dir IS surfaced by a ranked query.
    let hit = query::top_match(store, &spec, "a/b/c").unwrap();
    assert_eq!(hit.unwrap(), root.join("a").join("b").join("c"));
}

#[test]
fn ranked_query_surfaces_indexed_dir_floored_below_real_visits() {
    assert_indexed_ranks_below_real_visit(&DirFrecencyDb::open_in_memory().unwrap());
    assert_indexed_ranks_below_real_visit(&MemDirStore::new());
}

#[test]
fn watcher_event_core_routes_create_and_remove() {
    let (_tmp, root) = fixture();
    let cfg = cfg_for(&root, 3);
    let store = MemDirStore::new();
    let ig = IgnoreSet::from_config(&cfg);

    // A created dir is upserted…
    let created = root.join("a").join("new");
    fs::create_dir(&created).unwrap();
    let outcome = indexer::apply_event_path(&store, &cfg.roots, &ig, &created).unwrap();
    assert_eq!(outcome, EventOutcome::Upserted);
    assert_eq!(store.entries().unwrap().len(), 1);

    // …a plain file is not…
    let file = root.join("file.txt");
    let outcome = indexer::apply_event_path(&store, &cfg.roots, &ig, &file).unwrap();
    assert_eq!(outcome, EventOutcome::Skipped);

    // …an ignored path is not…
    let ignored = root.join("node_modules").join("x");
    let outcome = indexer::apply_event_path(&store, &cfg.roots, &ig, &ignored).unwrap();
    assert_eq!(outcome, EventOutcome::Skipped);

    // …and a removed dir takes its subtree out of `discovered`.
    fs::remove_dir(&created).unwrap();
    let outcome = indexer::apply_event_path(&store, &cfg.roots, &ig, &created).unwrap();
    assert_eq!(outcome, EventOutcome::Removed);
    assert!(store.entries().unwrap().is_empty());
}

#[test]
fn config_defaults_and_serde_round_trip() {
    // Tiered defaults: bare is off/empty; prescribed is the curated surface.
    let bare = WadachiConfig::bare();
    assert!(!bare.indexer.enabled);
    assert!(bare.indexer.roots.is_empty());
    assert!(bare.indexer.ignore_names.is_empty());

    let cfg = WadachiConfig::prescribed_default();
    assert!(cfg.indexer.enabled);
    assert!(!cfg.indexer.index_hidden);
    assert_eq!(cfg.indexer.debounce_ms, 500);
    assert_eq!(cfg.indexer.rewalk_interval_secs, 900);
    assert!(cfg.indexer.concurrency >= 1);
    for name in [".git", "node_modules", "target", ".direnv", ".cache", "Library", ".Trash"] {
        assert!(
            cfg.indexer.ignore_names.iter().any(|n| n == name),
            "prescribed ignore set must contain {name}"
        );
    }
    if let Some(home) = dirs::home_dir() {
        assert!(
            cfg.indexer.roots.contains(&IndexerRoot::new(home.join("code"), 6)),
            "~/code must be a default root (deep)"
        );
        assert!(
            cfg.indexer.roots.contains(&IndexerRoot::new(home, 2)),
            "$HOME must be a default root (shallow)"
        );
    }

    // Serde round-trip is lossless for every tier.
    for cfg in [bare, WadachiConfig::discovered(), cfg] {
        let json = serde_json::to_string(&cfg).unwrap();
        let back: WadachiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }
}
