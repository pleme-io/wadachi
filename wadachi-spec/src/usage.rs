//! Usage frecency for **text entries** — command lines, prompts, anything a
//! person re-runs — as opposed to [`crate::apply`]'s path-shaped ranking.
//!
//! Lifted out of skim-tab's `usage_rank` (the fleet's Ctrl-R feed) so every
//! history picker orders its feed the same way: skim-tab's shell history and
//! arnes's prompt history both rank here and cannot drift.
//!
//! ## Why not [`crate::apply`]
//!
//! That matcher is *path-shaped* (components, basename, ancestors). A command
//! line is not a path: feeding one to it mis-tokenizes on `/`, measured as an
//! 8× distortion between two commands with identical visit counts
//! (`grep -rn foo lib` → 4.0 versus `grep -rn foo src/lib.rs` → 0.5). So this
//! module borrows only the **decay** ([`DecayKind::HyperbolicDays`]) and owns
//! the accumulation and the ordering. Needle matching stays with the picker.
//!
//! ## The muscle-memory invariant
//!
//! Hyperbolic-days decay is nearly flat within a day, so a line run forty
//! times last week outscores the one just run. Frecency alone would make the
//! dominant history use — *re-run what I just ran* — worse.
//! [`RankOptions::pin_most_recent`] places the newest entry first
//! unconditionally, and frecency orders everything beneath it.
//!
//! The source format is the caller's: skim-tab parses zsh extended history
//! lines into `(timestamp, text)` pairs, arnes reads its own JSONL. Both feed
//! [`accumulate`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::DecayKind;

/// Seconds in a day, as `f64` — the unit [`DecayKind`] decays in.
const SECS_PER_DAY: f64 = 86_400.0;

/// How a feed is ordered.
///
/// `Recency` is kept as a named selection, not deleted, so the pre-frecency
/// order stays reachable for anyone who prefers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RankMode {
    /// Time-decayed frequency: `Σ 1/(1 + age_days)` over every occurrence.
    #[default]
    Frecency,
    /// Pure positional recency over deduplicated entries.
    Recency,
}

/// Knobs for one ranking pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankOptions {
    /// Ordering to apply.
    pub mode: RankMode,
    /// Place the most recently used entry first regardless of score.
    pub pin_most_recent: bool,
}

impl Default for RankOptions {
    fn default() -> Self {
        Self {
            mode: RankMode::default(),
            pin_most_recent: true,
        }
    }
}

/// One distinct entry and every time it was observed. Re-running an entry
/// appends rather than overwrites, which is what keeps frequency recoverable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageEntry {
    /// The entry text, exactly as a picker will show it.
    pub command: String,
    /// Unix-epoch seconds of each occurrence that carried a timestamp,
    /// ascending. Empty when the source has none.
    pub timestamps: Vec<i64>,
    /// Position of the most recent occurrence in the source. Orders
    /// `Recency` and breaks ties in `Frecency`, so an untimestamped source
    /// still ranks sensibly.
    pub last_ordinal: usize,
    /// Total occurrences, timestamped or not. Always `>= timestamps.len()`.
    pub occurrences: usize,
}

impl UsageEntry {
    /// `Σ 1/(1 + age_days)` across every timestamped occurrence. `0.0` when
    /// nothing is timestamped; such entries fall back to `last_ordinal`.
    #[must_use]
    pub fn frecency(&self, now_unix: i64) -> f64 {
        self.timestamps
            .iter()
            .map(|ts| {
                #[allow(clippy::cast_precision_loss)]
                let age_days = ((now_unix - ts) as f64 / SECS_PER_DAY).max(0.0);
                DecayKind::HyperbolicDays.decay(age_days, 0.0)
            })
            .sum()
    }
}

/// Accumulate `(timestamp, text)` occurrences, oldest first, into one entry
/// per distinct text, keeping occurrence counts and timestamps. Empty texts
/// are skipped.
#[must_use]
pub fn accumulate<'a>(occurrences: impl IntoIterator<Item = (Option<i64>, &'a str)>) -> Vec<UsageEntry> {
    let mut index: HashMap<&'a str, usize> = HashMap::new();
    let mut entries: Vec<UsageEntry> = Vec::new();
    for (ordinal, (ts, text)) in occurrences.into_iter().filter(|(_, t)| !t.is_empty()).enumerate() {
        let slot = *index.entry(text).or_insert_with(|| {
            entries.push(UsageEntry {
                command: text.to_owned(),
                timestamps: Vec::new(),
                last_ordinal: ordinal,
                occurrences: 0,
            });
            entries.len() - 1
        });
        let entry = &mut entries[slot];
        entry.occurrences += 1;
        entry.last_ordinal = ordinal;
        if let Some(ts) = ts {
            entry.timestamps.push(ts);
        }
    }
    entries
}

/// Order `entries` best-first, keeping the entries so a caller can report
/// why each one landed where it did.
#[must_use]
pub fn rank_entries(mut entries: Vec<UsageEntry>, opts: RankOptions, now_unix: i64) -> Vec<UsageEntry> {
    match opts.mode {
        RankMode::Recency => entries.sort_by_key(|e| std::cmp::Reverse(e.last_ordinal)),
        RankMode::Frecency => entries.sort_by(|a, b| {
            b.frecency(now_unix)
                .total_cmp(&a.frecency(now_unix))
                .then_with(|| b.last_ordinal.cmp(&a.last_ordinal))
        }),
    }
    if opts.pin_most_recent
        && let Some(newest) = entries
            .iter()
            .enumerate()
            .max_by_key(|(_, e)| e.last_ordinal)
            .map(|(i, _)| i)
        && newest != 0
    {
        let entry = entries.remove(newest);
        entries.insert(0, entry);
    }
    entries
}

/// [`rank_entries`], reduced to the texts a picker is fed.
#[must_use]
pub fn rank(entries: Vec<UsageEntry>, opts: RankOptions, now_unix: i64) -> Vec<String> {
    rank_entries(entries, opts, now_unix).into_iter().map(|e| e.command).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain<'a>(texts: &[&'a str]) -> Vec<(Option<i64>, &'a str)> {
        texts.iter().map(|t| (None, *t)).collect()
    }

    fn unpinned(mode: RankMode) -> RankOptions {
        RankOptions { mode, pin_most_recent: false }
    }

    #[test]
    fn repeats_accumulate_instead_of_overwriting() {
        let entries = accumulate(plain(&["ls", "cd /tmp", "ls", "ls"]));
        let ls = entries.iter().find(|e| e.command == "ls").unwrap();
        assert_eq!(ls.occurrences, 3);
        assert_eq!(ls.last_ordinal, 3);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn timestamps_are_kept_per_occurrence_and_empty_texts_skipped() {
        let entries = accumulate([(Some(100), "git status"), (Some(1), ""), (Some(200), "git status"), (None, "git status")]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamps, vec![100, 200]);
        assert_eq!(entries[0].occurrences, 3);
        assert_eq!(entries[0].last_ordinal, 2, "the skipped empty text takes no ordinal");
    }

    #[test]
    fn frequency_and_recency_both_raise_the_score() {
        let now = 1_000_000_000;
        let day = 86_400;
        let fresh = accumulate([(Some(now - day), "x")])[0].frecency(now);
        let stale = accumulate([(Some(now - 100 * day), "x")])[0].frecency(now);
        let thrice = accumulate([(Some(now - day), "x"), (Some(now - day), "x"), (Some(now - day), "x")])[0].frecency(now);
        assert!(fresh > stale);
        assert!(thrice > fresh * 2.0);
        assert!(accumulate([(None, "x")])[0].frecency(now).abs() < f64::EPSILON);
    }

    #[test]
    fn frequent_rises_above_a_rarer_newer_entry() {
        let now = 2_000_000_000;
        let old = now - 3 * 86_400;
        let occ = [(Some(old), "git commit --amend"), (Some(old), "git commit --amend"), (Some(old), "git commit --amend"), (Some(now - 2 * 86_400), "one-off")];
        let feed = rank(accumulate(occ), unpinned(RankMode::Frecency), now);
        assert_eq!(feed[0], "git commit --amend");
    }

    /// The guard on the dominant history reflex: without the pin, forty uses
    /// outrank the entry just run.
    #[test]
    fn the_newest_entry_is_pinned_above_a_far_more_frequent_one() {
        let now = 2_000_000_000;
        let mut occ: Vec<(Option<i64>, &str)> = (0..40).map(|_| (Some(now - 86_400), "git status")).collect();
        occ.push((Some(now), "cargo test"));
        assert_eq!(rank(accumulate(occ.clone()), RankOptions::default(), now)[0], "cargo test");
        assert_eq!(rank(accumulate(occ), unpinned(RankMode::Frecency), now)[0], "git status");
    }

    #[test]
    fn recency_is_newest_first_and_a_rerun_moves_up() {
        assert_eq!(rank(accumulate(plain(&["one", "two", "three"])), unpinned(RankMode::Recency), 0), ["three", "two", "one"]);
        assert_eq!(rank(accumulate(plain(&["ls", "cd /tmp", "ls"])), unpinned(RankMode::Recency), 0), ["ls", "cd /tmp"]);
        assert!(rank(Vec::new(), RankOptions::default(), 0).is_empty());
    }

    /// A `/` is text, not a path separator: this is the distortion routing
    /// through the path matcher would bring back.
    #[test]
    fn a_slash_does_not_reweight_an_entry() {
        let now = 2_000_000_000;
        let t = Some(now - 86_400);
        let a = accumulate([(t, "grep -rn foo lib")])[0].frecency(now);
        let b = accumulate([(t, "grep -rn foo src/lib.rs")])[0].frecency(now);
        assert!((a - b).abs() < f64::EPSILON);
    }

    #[test]
    fn rank_mode_spells_lowercase() {
        assert_eq!(serde_json::to_string(&RankMode::Recency).unwrap(), "\"recency\"");
        assert_eq!(serde_json::from_str::<RankMode>("\"frecency\"").unwrap(), RankMode::Frecency);
    }
}
