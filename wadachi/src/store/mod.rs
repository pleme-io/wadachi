//! The directory store — the shared on-disk bus every consumer reads/writes.
//!
//! frost writes a row on every `cd`; mado writes on every OSC-7 cwd; the
//! background indexer upserts discovered dirs; skim-cd and the MCP layer read.
//! No IPC, no daemon required for the core loop — one `SQLite` file (WAL mode).
//!
//! Ranking never lives here: [`DirStore::entries`] loads the candidates and
//! [`crate::query`] feeds them through `wadachi_spec::apply`. The store owns
//! storage; the spec owns the formula.

mod mem;
mod sqlite;

pub use mem::MemDirStore;
pub use sqlite::DirFrecencyDb;

use wadachi_spec::DirEntry;

/// Abstraction over directory-frecency storage. The two impls
/// ([`DirFrecencyDb`] real, [`MemDirStore`] in-memory) are the test seam —
/// everything above the store can be exercised without touching disk.
pub trait DirStore {
    /// Record a real visit to `path` (absolute) at the current time.
    ///
    /// # Errors
    /// Propagates storage failures.
    fn record(&self, path: &str) -> anyhow::Result<()>;

    /// Record `path` as *discovered* by the indexer (never a real visit).
    /// Idempotent — re-discovering an existing path is a no-op.
    ///
    /// # Errors
    /// Propagates storage failures.
    fn record_discovered(&self, path: &str) -> anyhow::Result<()>;

    /// Load every candidate directory with its visit timestamps, ready for
    /// ranking. Discovered-only dirs come back with empty `visits` and
    /// `discovered_only = true`.
    ///
    /// # Errors
    /// Propagates storage failures.
    fn entries(&self) -> anyhow::Result<Vec<DirEntry>>;

    /// Every discovered path equal to or under `prefix` (path-component-wise,
    /// not raw string prefix). The indexer's prune pass reads this scoped to
    /// the root it just re-walked, so staleness stays bounded per root.
    ///
    /// # Errors
    /// Propagates storage failures.
    fn discovered_under(&self, prefix: &str) -> anyhow::Result<Vec<String>>;

    /// Remove `path` *and its entire subtree* from the discovered set (a
    /// deleted dir takes its children with it). Idempotent — removing an
    /// absent path is a no-op. Never touches `visits`.
    ///
    /// # Errors
    /// Propagates storage failures.
    fn remove_discovered(&self, path: &str) -> anyhow::Result<()>;
}

/// `true` when `candidate` is `prefix` itself or lives under it as a path
/// (component-boundary-aware: `/a/bc` is NOT under `/a/b`).
#[must_use]
pub fn path_is_same_or_under(candidate: &str, prefix: &str) -> bool {
    candidate == prefix
        || candidate
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}
