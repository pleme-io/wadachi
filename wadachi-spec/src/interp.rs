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
                    acc.ages = acc
                        .visits
                        .iter()
                        .map(|t| age_days(now, *t))
                        .collect();
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
                    let recency: f64 = acc.decayed.iter().sum();
                    #[allow(clippy::cast_precision_loss)]
                    let freq = acc.freq as f64;
                    acc.score = spec.recency_weight * recency + spec.freq_weight * freq;
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
            RankPhase::SortDesc => {
                let set = require(working.as_mut(), "SortDesc")?;
                set.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
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

fn require<'a>(set: Option<&'a mut Vec<Acc>>, phase: &str) -> Result<&'a mut Vec<Acc>, SpecError> {
    set.ok_or_else(|| SpecError::Interp {
        phase: phase.to_owned(),
        reason: "phase ran before `LoadEntries` seeded the working set".to_owned(),
    })
}

fn age_days(now: NaiveDateTime, then: NaiveDateTime) -> f64 {
    let secs = (now - then).num_seconds();
    #[allow(clippy::cast_precision_loss)]
    let days = secs as f64 / 86_400.0;
    days
}
