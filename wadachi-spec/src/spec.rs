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

/// How well a needle matches a path — the typed vocabulary of *matching*,
/// which used to be an untyped `path.contains(needle)` living outside the
/// spec entirely.
///
/// Declaration order is **worst → best**, and the derived [`Ord`] is what
/// [`MatchProfile::require_at_least`] compares against, so "which matches
/// are good enough" is a total order by construction rather than a
/// hand-maintained set of `if`s.
///
/// The historical behavior is exactly [`MatchKind::SubstringAnywhere`]: it
/// is why `cd ni` used to offer `…/akeyless-commu`**`ni`**`ty/` and
/// `…/forta`**`ni`**`x/` — the needle matched the *middle of a word* in an
/// ancestor directory nobody had ever visited.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchKind {
    /// The needle occurs somewhere in the full path — the loosest possible
    /// match, and the source of nearly all historical noise.
    SubstringAnywhere,
    /// Some non-final path component starts with the needle
    /// (`wad` → `…/wadachi/wadachi-spec/specs`).
    ComponentPrefix,
    /// The needle's characters occur in order within the basename
    /// (`wdc` → `wadachi`). Fuzzy, but anchored to the thing being named.
    BasenameSubsequence,
    /// The basename starts with the needle (`wad` → `wadachi`).
    BasenamePrefix,
    /// The basename equals the needle (`nix` → `nix`).
    BasenameExact,
}

impl MatchKind {
    /// Every variant, worst → best. The verification matrix asserts this is
    /// total, so a new variant cannot land without a row.
    pub const ALL: &'static [MatchKind] = &[
        MatchKind::SubstringAnywhere,
        MatchKind::ComponentPrefix,
        MatchKind::BasenameSubsequence,
        MatchKind::BasenamePrefix,
        MatchKind::BasenameExact,
    ];
}

/// How each [`MatchKind`] weights a candidate's score, and the floor below
/// which a match is not a match at all.
///
/// This is the typed replacement for the unmodeled substring filter. Because
/// it is *data*, the fleet's feel is authored in `frecency.lisp` rather than
/// compiled into a filter expression.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct MatchProfile {
    /// Multiplier applied to a candidate whose best match is
    /// [`MatchKind::BasenameExact`].
    pub basename_exact: f64,
    /// Multiplier for [`MatchKind::BasenamePrefix`].
    pub basename_prefix: f64,
    /// Multiplier for [`MatchKind::BasenameSubsequence`].
    pub basename_subsequence: f64,
    /// Multiplier for [`MatchKind::ComponentPrefix`].
    pub component_prefix: f64,
    /// Multiplier for [`MatchKind::SubstringAnywhere`].
    pub substring_anywhere: f64,
    /// The weakest match kind still considered a match. Anything below this
    /// is dropped outright — *not* down-weighted — so a needle can never be
    /// answered by a coincidence in the middle of an ancestor's name.
    pub require_at_least: MatchKind,
}

impl MatchProfile {
    /// The fleet default: strongly prefer the basename, and refuse
    /// [`MatchKind::SubstringAnywhere`] entirely.
    #[must_use]
    pub fn anchored() -> Self {
        Self {
            basename_exact: 8.0,
            basename_prefix: 4.0,
            basename_subsequence: 1.0,
            component_prefix: 0.5,
            substring_anywhere: 0.1,
            require_at_least: MatchKind::ComponentPrefix,
        }
    }

    /// The pre-spec behavior, preserved as a named instance so the old feel
    /// is a *selection* rather than a deletion (MODULARIZE, DON'T DELETE):
    /// every kind weighted equally and nothing excluded.
    #[must_use]
    pub fn substring_legacy() -> Self {
        Self {
            basename_exact: 1.0,
            basename_prefix: 1.0,
            basename_subsequence: 1.0,
            component_prefix: 1.0,
            substring_anywhere: 1.0,
            require_at_least: MatchKind::SubstringAnywhere,
        }
    }

    /// The multiplier for `kind`.
    #[must_use]
    pub fn weight(&self, kind: MatchKind) -> f64 {
        match kind {
            MatchKind::BasenameExact => self.basename_exact,
            MatchKind::BasenamePrefix => self.basename_prefix,
            MatchKind::BasenameSubsequence => self.basename_subsequence,
            MatchKind::ComponentPrefix => self.component_prefix,
            MatchKind::SubstringAnywhere => self.substring_anywhere,
        }
    }

    /// Classify how `needle` matches `path`, returning the *best* applicable
    /// [`MatchKind`], or `None` when the needle does not match at all or
    /// matches only below [`Self::require_at_least`].
    ///
    /// A needle containing `/` is read as an ordered path fragment: every
    /// leading fragment must match an ancestor component in order, and the
    /// final fragment is classified against the basename. This is what makes
    /// `cd pleme-io/wad` mean what an operator expects.
    ///
    /// Matching is case-insensitive; an empty needle matches everything at
    /// [`MatchKind::BasenameExact`] (the "no filter" identity).
    #[must_use]
    pub fn classify(&self, needle: &str, path: &std::path::Path) -> Option<MatchKind> {
        let kind = Self::classify_raw(needle, path)?;
        (kind >= self.require_at_least).then_some(kind)
    }

    /// [`Self::classify`] without the `require_at_least` floor — the pure
    /// classifier, exposed so the matrix can assert every kind is reachable.
    #[must_use]
    pub fn classify_raw(needle: &str, path: &std::path::Path) -> Option<MatchKind> {
        if needle.is_empty() {
            return Some(MatchKind::BasenameExact);
        }
        let lower_path = path.to_string_lossy().to_lowercase();
        let needle = needle.to_lowercase();

        let components: Vec<String> = path
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
                _ => None,
            })
            .collect();
        let basename = components.last()?;

        // Split the needle into ordered fragments. All but the last must be
        // satisfied by ancestor components, in order.
        let fragments: Vec<&str> = needle.split('/').filter(|f| !f.is_empty()).collect();
        let Some((last, leading)) = fragments.split_last() else {
            return Some(MatchKind::BasenameExact);
        };

        let ancestors = &components[..components.len().saturating_sub(1)];
        let mut cursor = 0usize;
        for frag in leading {
            match ancestors[cursor..].iter().position(|c| c.contains(frag)) {
                Some(hit) => cursor += hit + 1,
                None => return None,
            }
        }

        if basename == last {
            Some(MatchKind::BasenameExact)
        } else if basename.starts_with(last) {
            Some(MatchKind::BasenamePrefix)
        } else if ancestors[cursor..].iter().any(|c| c.starts_with(last)) {
            Some(MatchKind::ComponentPrefix)
        } else if is_subsequence(last, basename) {
            Some(MatchKind::BasenameSubsequence)
        } else if leading.is_empty() && lower_path.contains(last) {
            Some(MatchKind::SubstringAnywhere)
        } else {
            None
        }
    }
}

impl Default for MatchProfile {
    fn default() -> Self {
        Self::anchored()
    }
}

/// `true` when every char of `needle` appears in `hay` in order.
fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut chars = hay.chars();
    needle.chars().all(|n| chars.any(|h| h == n))
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
    /// Drop candidates the needle does not match, and scale the survivors by
    /// their [`MatchKind`]'s weight from [`FrecencyRankingSpec::matching`].
    ///
    /// This is the phase that pulls *matching* inside the spec. It is a
    /// no-op when the needle is empty, which is what keeps the published
    /// [`crate::apply`] signature behavior-preserving.
    MatchNeedle,
    /// Drop a candidate when an already-kept, higher-ranked candidate is one
    /// of its ancestors, retaining at most `keep` descendants per kept root.
    ///
    /// Without this, one repository floods the result set with its own
    /// subdirectories — measured: 6 of 8 `cd wad` slots were
    /// `wadachi/`, `wadachi/wadachi-spec/`, `…/specs/`, `…/src/`, `…/tests/`.
    ///
    /// Requires a preceding `SortDesc` to be meaningful, and is a no-op when
    /// the needle is empty.
    CollapseDescendants { keep: usize },
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
    /// How a needle matches a path, and how much each match kind is worth.
    ///
    /// `#[serde(default)]` so a spec serialized before matching was modeled
    /// still deserializes — it simply adopts the anchored default.
    #[serde(default)]
    pub matching: MatchProfile,
    /// The ordered ranking pipeline.
    pub phases: Vec<RankPhase>,
}

impl FrecencyRankingSpec {
    /// The canonical pipeline shared by every named instance.
    ///
    /// `MatchNeedle` sits after `FloorIndexed` so it scales the *settled*
    /// score (including the discovered-only floor), and `CollapseDescendants`
    /// sits after `SortDesc` because "is one of my ancestors already kept?"
    /// is only meaningful in rank order.
    #[must_use]
    pub fn canonical_phases() -> Vec<RankPhase> {
        vec![
            RankPhase::LoadEntries,
            RankPhase::ComputeAge,
            RankPhase::ApplyDecay,
            RankPhase::Combine,
            RankPhase::FloorIndexed,
            RankPhase::MatchNeedle,
            RankPhase::SortDesc,
            RankPhase::CollapseDescendants { keep: 0 },
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
            matching: MatchProfile::anchored(),
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
            matching: MatchProfile::anchored(),
            phases: Self::canonical_phases(),
        }
    }

    /// The behavior wadachi shipped before matching was modeled: unanchored
    /// substring matching with no descendant collapse. Kept as a *named
    /// instance* so the old feel is selectable rather than deleted, and so
    /// the matrix can pin exactly what changed.
    #[must_use]
    pub fn substring_legacy() -> Self {
        Self {
            name: "substring-legacy".to_owned(),
            decay: DecayKind::HyperbolicDays,
            half_life_days: 0.0,
            freq_weight: 0.0,
            recency_weight: 1.0,
            indexed_epsilon: 0.001,
            matching: MatchProfile::substring_legacy(),
            phases: vec![
                RankPhase::LoadEntries,
                RankPhase::ComputeAge,
                RankPhase::ApplyDecay,
                RankPhase::Combine,
                RankPhase::FloorIndexed,
                RankPhase::MatchNeedle,
                RankPhase::SortDesc,
                RankPhase::TopK { n: 50 },
            ],
        }
    }

    /// Look an instance up by name (the set the authored `frecency.lisp`
    /// declares). Returns `None` for an unknown name.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "skimtab-parity" => Some(Self::skimtab_parity()),
            "zoxide-parity" => Some(Self::zoxide_parity()),
            "substring-legacy" => Some(Self::substring_legacy()),
            _ => None,
        }
    }

    /// Every instance the spec ships (drives the verification matrix).
    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::skimtab_parity(),
            Self::zoxide_parity(),
            Self::substring_legacy(),
        ]
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
