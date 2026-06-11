//! `wadachi` (轍) — a native typed-Rust **directory frecency** primitive for
//! the pleme-io fleet: it records the directories you walk and serves a fuzzy,
//! frecency-ranked `cd` from a `SQLite` store that frost, skim-cd and mado all
//! share. A zoxide *replacement*, not a wrapper.
//!
//! ## The in-process facade — one writer, many readers
//!
//! Every consumer links this crate and calls the facade directly (no forking a
//! `wadachi` subprocess, no PATH dependency):
//!
//! - **frost** (the shell) is the *sole recorder* — it calls [`record`] at its
//!   `chdir` chokepoint and [`resolve`] for smart-cd.
//! - **skim-cd** and **mado** are *readers* — they call [`top_n`] to rank a
//!   picker; they never record (the shell already did).
//!
//! Because the facade resolves the same well-known [`config::default_db_path`]
//! everywhere, the three tools converge on one store with **zero config** — the
//! "it just works together" guarantee. The hot path is deliberately cheap: it
//! opens the store and does one operation, never touching the filesystem-walking
//! [`config::WadachiConfig::discovered`] tier (that's for `config-show` + the
//! background indexer).
//!
//! ```
//! use wadachi_spec::FrecencyRankingSpec;
//! use pleme_io_wadachi::{store::{DirStore, MemDirStore}, query};
//! let store = MemDirStore::new();
//! store.record("/code/github/pleme-io/wadachi").unwrap();
//! let hit = query::top_match(&store, &FrecencyRankingSpec::skimtab_parity(), "wadachi").unwrap();
//! assert_eq!(hit.unwrap().to_str().unwrap(), "/code/github/pleme-io/wadachi");
//! ```

pub mod config;
pub mod indexer;
pub mod query;
pub mod store;

pub use config::{ConfigTier, IndexerConfig, IndexerRoot, WadachiConfig};
pub use store::{DirFrecencyDb, DirStore, MemDirStore};

// Re-export the shared spec so consumers depend on one crate.
pub use wadachi_spec;

use std::path::PathBuf;

use wadachi_spec::{FrecencyRankingSpec, RankedDir};

/// The store path the per-`cd` hot path uses: `WADACHI_DB` or the XDG default.
/// Cheap — never walks the filesystem (unlike the `discovered()` tier).
#[must_use]
pub fn runtime_db_path() -> PathBuf {
    std::env::var("WADACHI_DB")
        .map_or_else(|_| config::default_db_path(), PathBuf::from)
}

/// The ranking spec the hot path uses: `WADACHI_RANKING` or the fleet default.
fn runtime_spec() -> FrecencyRankingSpec {
    std::env::var("WADACHI_RANKING")
        .ok()
        .and_then(|r| FrecencyRankingSpec::by_name(&r))
        .unwrap_or_else(FrecencyRankingSpec::skimtab_parity)
}

/// Record a visit to `path` into the default store. The shell's per-`cd` hook —
/// in-process (no fork), best-effort (callers ignore the error so frecency can
/// never break navigation).
///
/// # Errors
/// Propagates store-open / write failures (callers typically ignore them).
pub fn record(path: &str) -> anyhow::Result<()> {
    let store = DirFrecencyDb::open(&runtime_db_path())?;
    store.record(path)
}

/// The single best directory matching `needle`, or `None` — the shell's
/// smart-cd resolver (called only when a literal `cd` fails).
///
/// # Errors
/// Propagates store / interpreter failures.
pub fn resolve(needle: &str) -> anyhow::Result<Option<PathBuf>> {
    let store = DirFrecencyDb::open(&runtime_db_path())?;
    query::top_match(&store, &runtime_spec(), needle)
}

/// The top `limit` frecency-ranked directories matching `needle` — the read
/// side for pickers (skim-cd) and the mado overlay.
///
/// # Errors
/// Propagates store / interpreter failures.
pub fn top_n(needle: &str, limit: usize) -> anyhow::Result<Vec<RankedDir>> {
    let store = DirFrecencyDb::open(&runtime_db_path())?;
    query::top_n(&store, &runtime_spec(), needle, limit)
}
