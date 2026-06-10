//! Typed border for the frecency-ranking algorithm.
//!
//! The ranking algorithm is an *algorithmic primitive* — it ships as the
//! pleme-io TYPED-SPEC + INTERPRETER TRIPLET: this typed Rust border, an
//! authored Lisp spec ([`specs/frecency.lisp`](../specs/frecency.lisp)) that
//! declares the canonical instances as data, and the interpreter in
//! [`crate::interp`] that walks the phases against a mockable
//! [`crate::env::FrecencyEnvironment`].
//!
//! Every consumer — wadachi's directory store, skim-tab's command history,
//! a future zoxide import — drives a *named instance of this one spec*, so
//! there is exactly one ranking formula and the consumers cannot drift.
//!
//! ## Authoring surface
//!
//! ```lisp
//! (deffrecency-ranking
//!   :name "skimtab-parity"
//!   :decay HyperbolicDays
//!   :freq-weight 0.0
//!   :recency-weight 1.0
//!   :indexed-epsilon 0.001
//!   :phases ((:kind LoadEntries) (:kind ComputeAge) (:kind ApplyDecay)
//!            (:kind Combine) (:kind FloorIndexed) (:kind SortDesc)
//!            (:kind TopK :n 50)))
//! ```
//!
//! > **Note.** The `#[derive(DeriveTataraDomain)]` authoring macro (which
//! > makes `(deffrecency-ranking …)` a first-class tatara-lisp keyword) is a
//! > fast-follow — wiring it pulls the full tatara-lisp closure as a git dep.
//! > Until then the canonical instances live as the typed constructors below
//! > and the `.lisp` file is the spec-of-record they mirror.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How a single visit's age (in days) decays into a recency weight.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecayKind {
    /// `1 / (1 + age_days)` — skim-tab's proven hyperbolic decay. Recent
    /// visits contribute ~1.0; old visits tail off gently. The fleet default.
    HyperbolicDays,
    /// `2^(-age_days / half_life_days)` — exponential half-life, the shape
    /// closest to zoxide's recency feel.
    ExpHalfLife,
    /// zoxide-style time buckets: within the hour ×4, the day ×2, the week
    /// ×0.5, else ×0.25 — a coarse recency factor summed per visit.
    ZoxideLogBuckets,
}

impl DecayKind {
    /// Decay one visit of the given age (days) to a recency weight.
    #[must_use]
    pub fn decay(self, age_days: f64, half_life_days: f64) -> f64 {
        let age = age_days.max(0.0);
        match self {
            DecayKind::HyperbolicDays => 1.0 / (1.0 + age),
            DecayKind::ExpHalfLife => {
                let hl = if half_life_days <= 0.0 { 1.0 } else { half_life_days };
                2.0_f64.powf(-age / hl)
            }
            DecayKind::ZoxideLogBuckets => {
                if age < 1.0 / 24.0 {
                    4.0
                } else if age < 1.0 {
                    2.0
                } else if age < 7.0 {
                    0.5
                } else {
                    0.25
                }
            }
        }
    }
}

/// One step of the ranking pipeline. The interpreter walks these in order;
/// an unrecognized phase is a typed error, never a silent skip.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum RankPhase {
    /// Seed the working set from the input entries.
    LoadEntries,
    /// Compute each visit's age in days against `env.now()`.
    ComputeAge,
    /// Decay every age into a recency weight via [`DecayKind`].
    ApplyDecay,
    /// Combine recency (sum of decayed weights) and frequency (visit count)
    /// using `recency_weight` / `freq_weight`.
    Combine,
    /// Replace the score of discovered-only entries with `indexed_epsilon`
    /// so an indexed-but-never-visited dir is rankable yet can never outrank
    /// a single real visit.
    FloorIndexed,
    /// Sort the working set by score, descending.
    SortDesc,
    /// Keep only the top `n`.
    TopK { n: usize },
}

/// The full frecency-ranking algorithm as typed data — one named instance
/// per consumer feel.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FrecencyRankingSpec {
    /// Instance name (`"skimtab-parity"`, `"zoxide-parity"`, …).
    pub name: String,
    /// Per-visit recency decay shape.
    pub decay: DecayKind,
    /// Half-life in days, used by [`DecayKind::ExpHalfLife`].
    pub half_life_days: f64,
    /// Weight on the frequency term (visit count).
    pub freq_weight: f64,
    /// Weight on the recency term (sum of decayed visit weights).
    pub recency_weight: f64,
    /// Floor score assigned to discovered-only (indexed, never-visited) dirs.
    pub indexed_epsilon: f64,
    /// The ordered ranking pipeline.
    pub phases: Vec<RankPhase>,
}

impl FrecencyRankingSpec {
    /// The canonical pipeline shared by every named instance.
    #[must_use]
    pub fn canonical_phases() -> Vec<RankPhase> {
        vec![
            RankPhase::LoadEntries,
            RankPhase::ComputeAge,
            RankPhase::ApplyDecay,
            RankPhase::Combine,
            RankPhase::FloorIndexed,
            RankPhase::SortDesc,
            RankPhase::TopK { n: 50 },
        ]
    }

    /// `Σ 1/(1+age_days)` — recency-only. Behavior-identical to skim-tab's
    /// `frecency_score`, which is what makes adopting this spec in skim-tab a
    /// behavior-preserving extraction. The fleet default.
    #[must_use]
    pub fn skimtab_parity() -> Self {
        Self {
            name: "skimtab-parity".to_owned(),
            decay: DecayKind::HyperbolicDays,
            half_life_days: 0.0,
            freq_weight: 0.0,
            recency_weight: 1.0,
            indexed_epsilon: 0.001,
            phases: Self::canonical_phases(),
        }
    }

    /// `Σ 2^(-age/30d) + visits` — frequency × exponential half-life, the
    /// shape closest to operators' zoxide muscle memory.
    #[must_use]
    pub fn zoxide_parity() -> Self {
        Self {
            name: "zoxide-parity".to_owned(),
            decay: DecayKind::ExpHalfLife,
            half_life_days: 30.0,
            freq_weight: 1.0,
            recency_weight: 1.0,
            indexed_epsilon: 0.001,
            phases: Self::canonical_phases(),
        }
    }

    /// Look an instance up by name (the set the authored `frecency.lisp`
    /// declares). Returns `None` for an unknown name.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "skimtab-parity" => Some(Self::skimtab_parity()),
            "zoxide-parity" => Some(Self::zoxide_parity()),
            _ => None,
        }
    }

    /// Every instance the spec ships (drives the verification matrix).
    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![Self::skimtab_parity(), Self::zoxide_parity()]
    }
}

/// One candidate directory and the timestamps it was visited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Absolute path.
    pub path: PathBuf,
    /// Real-visit timestamps (UTC, naive). Empty for a discovered-only entry.
    pub visits: Vec<NaiveDateTime>,
    /// `true` when this dir was surfaced by the background indexer and never
    /// actually visited.
    pub discovered_only: bool,
}

/// A ranked directory and its frecency score.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedDir {
    /// Absolute path.
    pub path: PathBuf,
    /// Frecency score (higher = better).
    pub score: f64,
}
