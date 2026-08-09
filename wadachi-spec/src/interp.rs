//! The frecency interpreter — walks a [`FrecencyRankingSpec`]'s phases over a
//! set of [`DirEntry`] candidates and returns them ranked. The clock is the
//! only side effect, supplied via [`FrecencyEnvironment`], so the whole thing
//! is deterministic under test.

use std::cmp::Ordering;

use chrono::NaiveDateTime;

use crate::env::FrecencyEnvironment;
use crate::spec::{DirEntry, FrecencyRankingSpec, RankPhase, RankedDir};

/// A typed interpreter failure. Every unimplemented or out-of-order phase
/// surfaces here — never a silent wrong answer.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpecError {
    /// A phase ran against a working set that a prior phase should have set up.
    #[error("frecency interpreter failed at phase `{phase}`: {reason}")]
    Interp {
        /// The phase that failed.
        phase: String,
        /// Why it failed.
        reason: String,
    },
}

/// Per-entry accumulator threaded through the phases.
struct Acc {
    path: std::path::PathBuf,
    discovered_only: bool,
    freq: usize,
    visits: Vec<NaiveDateTime>,
    ages: Vec<f64>,
    decayed: Vec<f64>,
    score: f64,
}

/// Rank `entries` according to `spec`, using `env` for the current time.
///
/// Equivalent to [`apply_matched`] with an empty needle — the needle-driven
/// phases (`MatchNeedle`, `CollapseDescendants`) are defined to be no-ops in
/// that case, so this published signature keeps its original behavior exactly.
///
/// # Errors
/// Returns [`SpecError::Interp`] if the phase pipeline is malformed (e.g. a
/// compute phase runs before `LoadEntries`).
// `entries` is deliberately by-value: the published API hands ownership to
// the interpreter, and a spec may legally contain `LoadEntries` more than
// once (each load re-seeds from the same input). Switching to `&[DirEntry]`
// would be a breaking change to a crates.io-published signature.
#[allow(clippy::needless_pass_by_value)]
pub fn apply(
    spec: &FrecencyRankingSpec,
    entries: Vec<DirEntry>,
    env: &impl FrecencyEnvironment,
) -> Result<Vec<RankedDir>, SpecError> {
    apply_matched(spec, entries, "", env)
}

/// Rank `entries` against `needle` according to `spec`.
///
/// This is the full entry point: *matching* is a phase of the pipeline, not
/// a filter a caller applies beforehand. Before this existed, every consumer
/// pre-filtered with its own `path.contains(needle)` — an untyped step that
/// no spec governed and no matrix tested, and the reason `cd ni` answered
/// with `akeyless-commu`**`ni`**`ty`.
///
/// # Errors
/// Returns [`SpecError::Interp`] if the phase pipeline is malformed (e.g. a
/// compute phase runs before `LoadEntries`).
#[allow(clippy::needless_pass_by_value)]
pub fn apply_matched(
    spec: &FrecencyRankingSpec,
    entries: Vec<DirEntry>,
    needle: &str,
    env: &impl FrecencyEnvironment,
) -> Result<Vec<RankedDir>, SpecError> {
    let now = env.now();
    let mut working: Option<Vec<Acc>> = None;

    for phase in &spec.phases {
        match phase {
            RankPhase::LoadEntries => {
                working = Some(
                    entries
                        .iter()
                        .map(|e| Acc {
                            path: e.path.clone(),
                            discovered_only: e.discovered_only,
                            freq: e.visits.len(),
                            visits: e.visits.clone(),
                            ages: Vec::new(),
                            decayed: Vec::new(),
                            score: 0.0,
                        })
                        .collect(),
                );
            }
            RankPhase::ComputeAge => {
                let set = require(working.as_mut(), "ComputeAge")?;
                for acc in set.iter_mut() {
                    acc.ages = acc.visits.iter().map(|t| age_days(now, *t)).collect();
                }
            }
            RankPhase::ApplyDecay => {
                let set = require(working.as_mut(), "ApplyDecay")?;
                for acc in set.iter_mut() {
                    acc.decayed = acc
                        .ages
                        .iter()
                        .map(|a| spec.decay.decay(*a, spec.half_life_days))
                        .collect();
                }
            }
            RankPhase::Combine => {
                let set = require(working.as_mut(), "Combine")?;
                for acc in set.iter_mut() {
                    #[allow(clippy::cast_precision_loss)]
                    let freq = acc.freq as f64;
                    // The formula itself lives on the spec, not here — a
                    // consumer whose storage is a counter+timestamp rather than
                    // a visit log reaches the same code via `score_counted`.
                    acc.score = spec.combine_score(
                        &acc.decayed,
                        freq,
                        latest_decay(&acc.ages, &acc.decayed),
                    );
                }
            }
            RankPhase::FloorIndexed => {
                let set = require(working.as_mut(), "FloorIndexed")?;
                for acc in set.iter_mut() {
                    if acc.discovered_only {
                        acc.score = spec.indexed_epsilon;
                    }
                }
            }
            RankPhase::MatchNeedle => {
                let set = require(working.as_mut(), "MatchNeedle")?;
                match_needle(set, spec, needle);
            }
            RankPhase::CollapseDescendants { keep } => {
                let set = require(working.as_mut(), "CollapseDescendants")?;
                collapse_descendants(set, needle, *keep);
            }
            RankPhase::SortDesc => {
                let set = require(working.as_mut(), "SortDesc")?;
                // Ties break on the shorter path, then lexically — so a tie
                // between an ancestor and its descendant is settled in favor
                // of the ancestor, and the whole order is deterministic
                // rather than dependent on the store's row order.
                set.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| a.path.as_os_str().len().cmp(&b.path.as_os_str().len()))
                        .then_with(|| a.path.cmp(&b.path))
                });
            }
            RankPhase::TopK { n } => {
                let set = require(working.as_mut(), "TopK")?;
                set.truncate(*n);
            }
        }
    }

    let set = working.ok_or_else(|| SpecError::Interp {
        phase: "LoadEntries".to_owned(),
        reason: "spec had no LoadEntries phase — nothing to rank".to_owned(),
    })?;

    Ok(set
        .into_iter()
        .map(|acc| RankedDir {
            path: acc.path,
            score: acc.score,
        })
        .collect())
}

/// `RankPhase::MatchNeedle` — drop non-matches, scale survivors by their
/// match kind's weight. A no-op under an empty needle, which is what keeps
/// the published needle-free [`apply`] behavior-preserving.
fn match_needle(set: &mut Vec<Acc>, spec: &FrecencyRankingSpec, needle: &str) {
    if needle.is_empty() {
        return;
    }
    set.retain_mut(|acc| match spec.matching.classify(needle, &acc.path) {
        Some(kind) => {
            acc.score *= spec.matching.weight(kind);
            true
        }
        None => false,
    });
}

/// `RankPhase::CollapseDescendants` — keep at most `keep` entries beneath any
/// already-kept ancestor, walking in the current (rank) order. A no-op under
/// an empty needle.
fn collapse_descendants(set: &mut Vec<Acc>, needle: &str, keep: usize) {
    if needle.is_empty() {
        return;
    }
    let mut roots: Vec<(std::path::PathBuf, usize)> = Vec::new();
    set.retain_mut(|acc| {
        match roots
            .iter_mut()
            .find(|(root, _)| acc.path.starts_with(root))
        {
            Some((_, seen)) if *seen >= keep => false,
            Some((_, seen)) => {
                *seen += 1;
                true
            }
            None => {
                roots.push((acc.path.clone(), 0));
                true
            }
        }
    });
}

fn require<'a>(set: Option<&'a mut Vec<Acc>>, phase: &str) -> Result<&'a mut Vec<Acc>, SpecError> {
    set.ok_or_else(|| SpecError::Interp {
        phase: phase.to_owned(),
        reason: "phase ran before `LoadEntries` seeded the working set".to_owned(),
    })
}

/// The decayed weight of the **most recent** visit — the input
/// [`CombineKind::FreqTimesLatestDecay`] multiplies the visit count by.
///
/// Selected by smallest *age* rather than largest *weight*: "most recent" is a
/// fact about the visit, and reading it off the weights would silently depend
/// on every [`crate::DecayKind`] being monotonically non-increasing in age.
/// They all are today; a future one need not be, and this must not be the line
/// that quietly assumes it. `0.0` when there are no visits.
fn latest_decay(ages: &[f64], decayed: &[f64]) -> f64 {
    ages.iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(Ordering::Equal))
        .and_then(|(i, _)| decayed.get(i).copied())
        .unwrap_or(0.0)
}

fn age_days(now: NaiveDateTime, then: NaiveDateTime) -> f64 {
    let secs = (now - then).num_seconds();
    #[allow(clippy::cast_precision_loss)]
    let days = secs as f64 / 86_400.0;
    days
}
