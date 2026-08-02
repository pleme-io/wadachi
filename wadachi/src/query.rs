//! Ranked queries over a [`DirStore`] — the read side. This module owns *no*
//! matching or ranking policy: both are phases of the spec, walked by
//! `wadachi_spec::apply_matched`, so the formula AND the notion of "does this
//! needle match this path" each live in exactly one place.
//!
//! Historically this file carried its own `path.contains(needle)` filter,
//! applied *before* the interpreter ran. That untyped step was outside every
//! spec and every matrix, and it is why `cd ni` used to answer with
//! `…/akeyless-community/…` (the needle matched the middle of "commu-ni-ty").
//! Matching is now [`wadachi_spec::MatchKind`], authored in `frecency.lisp`.

use std::path::PathBuf;

use wadachi_spec::{apply_matched, FrecencyRankingSpec, RankedDir, RealEnvironment};

use crate::store::DirStore;

/// The top `limit` directories matching `needle`, ranked by `spec`.
///
/// How `needle` matches is `spec.matching` — see [`wadachi_spec::MatchProfile`].
/// An empty `needle` matches everything.
///
/// # Errors
/// Propagates store / interpreter failures.
pub fn top_n(
    store: &impl DirStore,
    spec: &FrecencyRankingSpec,
    needle: &str,
    limit: usize,
) -> anyhow::Result<Vec<RankedDir>> {
    let entries = store.entries()?;
    let ranked = apply_matched(spec, entries, needle, &RealEnvironment)?;
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
