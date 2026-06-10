//! Ranked queries over a [`DirStore`] — the read side. Filtering is plain
//! substring matching here; ranking is delegated to `wadachi_spec::apply` so
//! the formula lives in exactly one place.

use std::path::PathBuf;

use wadachi_spec::{apply, FrecencyRankingSpec, RankedDir, RealEnvironment};

use crate::store::DirStore;

/// The top `limit` directories matching `needle` (case-insensitive substring;
/// empty `needle` matches all), ranked by `spec`.
///
/// # Errors
/// Propagates store / interpreter failures.
pub fn top_n(
    store: &impl DirStore,
    spec: &FrecencyRankingSpec,
    needle: &str,
    limit: usize,
) -> anyhow::Result<Vec<RankedDir>> {
    let mut entries = store.entries()?;
    if !needle.is_empty() {
        let n = needle.to_lowercase();
        entries.retain(|e| e.path.to_string_lossy().to_lowercase().contains(&n));
    }
    let ranked = apply(spec, entries, &RealEnvironment)?;
    Ok(ranked.into_iter().take(limit).collect())
}

/// The single best directory matching `needle`, or `None`. This is what
/// frost's smart-cd resolver calls when a literal `cd` fails.
///
/// # Errors
/// Propagates store / interpreter failures.
pub fn top_match(
    store: &impl DirStore,
    spec: &FrecencyRankingSpec,
    needle: &str,
) -> anyhow::Result<Option<PathBuf>> {
    Ok(top_n(store, spec, needle, 1)?.into_iter().next().map(|r| r.path))
}
