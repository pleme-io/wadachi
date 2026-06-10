//! `wadachi` (轍) — a native typed-Rust **directory frecency** primitive for
//! the pleme-io fleet: it records the directories you walk and serves a fuzzy,
//! frecency-ranked `cd` from a SQLite store that frost, skim-cd and mado all
//! share. A zoxide *replacement*, not a wrapper — the zoxide binary is never
//! invoked at runtime.
//!
//! - The ranking formula is the shared [`wadachi_spec`] core (one spec, every
//!   consumer) — this crate never reimplements scoring.
//! - [`store`] is the on-disk bus ([`DirFrecencyDb`]) + its test seam
//!   ([`MemDirStore`]).
//! - [`query`] is the ranked read side ([`query::top_n`] / [`query::top_match`]).
//! - [`config`] is the typed config (db path + ranking instance).
//!
//! ```
//! use wadachi::{store::{DirStore, MemDirStore}, query};
//! use wadachi_spec::FrecencyRankingSpec;
//!
//! let store = MemDirStore::new();
//! store.record("/code/github/pleme-io/wadachi").unwrap();
//! let spec = FrecencyRankingSpec::skimtab_parity();
//! let hit = query::top_match(&store, &spec, "wadachi").unwrap();
//! assert_eq!(hit.unwrap().to_str().unwrap(), "/code/github/pleme-io/wadachi");
//! ```

pub mod config;
pub mod query;
pub mod store;

pub use config::WadachiConfig;
pub use store::{DirFrecencyDb, DirStore, MemDirStore};

// Re-export the shared spec so consumers depend on one crate.
pub use wadachi_spec;
