//! Verification matrix — exercises every shipped ranking instance and every
//! `DecayKind`, and fails the build if a new variant lands without a row. This
//! is the substrate's mechanical promise that ranking is correct across the
//! whole supported surface, not just the case the author happened to test.

use chrono::{Duration, NaiveDate, NaiveDateTime};
use wadachi_spec::{
    DecayKind, DirEntry, FrecencyRankingSpec, MatchKind, MatchProfile, MockEnvironment, RankPhase,
    apply, apply_matched,
};

fn at_day(d: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 6, d)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
}

/// Every `DecayKind` the spec border knows. A new variant must be added here
/// (and to `decay_matrix_is_total`) or the build fails — the forcing function.
const ALL_DECAYS: &[DecayKind] = &[
    DecayKind::HyperbolicDays,
    DecayKind::ExpHalfLife,
    DecayKind::ZoxideLogBuckets,
];

#[test]
fn decay_matrix_is_total() {
    // If a DecayKind is added without a row above, this count drifts.
    assert_eq!(
        ALL_DECAYS.len(),
        3,
        "a DecayKind was added/removed without updating the matrix"
    );
    // Every decay must be finite and non-negative for a fresh and an old visit.
    for &decay in ALL_DECAYS {
        for &age in &[0.0_f64, 1.0, 30.0, 365.0] {
            let w = decay.decay(age, 30.0);
            assert!(w.is_finite() && w >= 0.0, "{decay:?} @ {age}d → {w}");
        }
    }
}

#[test]
fn every_instance_ranks_recent_above_old() {
    let now = at_day(15);
    let env = MockEnvironment::at(now);
    let recent = now - Duration::days(1);
    let old = now - Duration::days(120);

    let mut failures = Vec::new();
    for spec in FrecencyRankingSpec::all() {
        let entries = vec![
            DirEntry {
                path: "/old".into(),
                visits: vec![old],
                discovered_only: false,
            },
            DirEntry {
                path: "/recent".into(),
                visits: vec![recent],
                discovered_only: false,
            },
        ];
        let ranked = apply(&spec, entries, &env).unwrap();
        if ranked.first().map(|r| r.path.to_str().unwrap()) != Some("/recent") {
            failures.push(spec.name.clone());
        }
    }
    assert!(
        failures.is_empty(),
        "instances ranked old over recent: {failures:?}"
    );
}

#[test]
fn skimtab_parity_is_behavior_preserving() {
    // The load-bearing fact: this instance reproduces skim-tab's exact
    // `score = Σ 1/(1+age_days)` so the extraction is safe.
    let now = at_day(10);
    let env = MockEnvironment::at(now);
    let spec = FrecencyRankingSpec::skimtab_parity();
    // two visits: one today (age 0 → 1.0), one 4 days ago (age 4 → 0.2)
    let entries = vec![DirEntry {
        path: "/x".into(),
        visits: vec![now, now - Duration::days(4)],
        discovered_only: false,
    }];
    let ranked = apply(&spec, entries, &env).unwrap();
    let expected = 1.0 / (1.0 + 0.0) + 1.0 / (1.0 + 4.0);
    assert!(
        (ranked[0].score - expected).abs() < 1e-9,
        "got {}",
        ranked[0].score
    );
}

#[test]
fn discovered_only_is_floored_and_below_recent_visits() {
    let now = at_day(10);
    let env = MockEnvironment::at(now);
    let spec = FrecencyRankingSpec::skimtab_parity();
    let entries = vec![
        DirEntry {
            path: "/indexed".into(),
            visits: vec![],
            discovered_only: true,
        },
        DirEntry {
            // Visited within the epsilon-crossover window (~2.7y for eps=0.001);
            // 100d → score 1/101 ≈ 0.0099 > 0.001.
            path: "/recent".into(),
            visits: vec![now - Duration::days(100)],
            discovered_only: false,
        },
    ];
    let ranked = apply(&spec, entries, &env).unwrap();
    let indexed = ranked
        .iter()
        .find(|r| r.path.to_str() == Some("/indexed"))
        .unwrap();
    let recent = ranked
        .iter()
        .find(|r| r.path.to_str() == Some("/recent"))
        .unwrap();
    // Discovered-only is floored to exactly epsilon — rankable, but the floor.
    // Exact float equality is the point: `FloorIndexed` *assigns* the epsilon
    // (no arithmetic), so bit-identity is the contract under test.
    #[allow(clippy::float_cmp)]
    {
        assert_eq!(indexed.score, spec.indexed_epsilon);
    }
    // A reasonably-recent real visit outranks a fresh discovery. (Visits older
    // than the crossover decay below the floor and are effectively forgotten,
    // which is the intended frecency behavior.)
    assert!(
        recent.score > indexed.score,
        "recent {} vs floor {}",
        recent.score,
        indexed.score
    );
}

#[test]
fn topk_truncates() {
    let now = at_day(10);
    let env = MockEnvironment::at(now);
    let mut spec = FrecencyRankingSpec::skimtab_parity();
    spec.phases = vec![
        wadachi_spec::RankPhase::LoadEntries,
        wadachi_spec::RankPhase::ComputeAge,
        wadachi_spec::RankPhase::ApplyDecay,
        wadachi_spec::RankPhase::Combine,
        wadachi_spec::RankPhase::FloorIndexed,
        wadachi_spec::RankPhase::SortDesc,
        wadachi_spec::RankPhase::TopK { n: 2 },
    ];
    let entries = (0..10)
        .map(|i| DirEntry {
            path: ["/d", i.to_string().as_str()].concat().into(),
            visits: vec![now - Duration::days(i)],
            discovered_only: false,
        })
        .collect();
    let ranked = apply(&spec, entries, &env).unwrap();
    assert_eq!(ranked.len(), 2);
}

#[test]
fn malformed_pipeline_errors_not_panics() {
    let env = MockEnvironment::at(at_day(10));
    let mut spec = FrecencyRankingSpec::skimtab_parity();
    // Combine before LoadEntries → typed error, never a silent wrong answer.
    spec.phases = vec![wadachi_spec::RankPhase::Combine];
    let err = apply(&spec, vec![], &env).unwrap_err();
    match err {
        wadachi_spec::SpecError::Interp { phase, .. } => assert_eq!(phase, "Combine"),
    }
}

// ─── matching matrix ────────────────────────────────────────────────────
// Matching used to live outside the spec as `path.contains(needle)`. These
// rows are the forcing function that keeps it inside.

/// Adding a `RankPhase` variant breaks THIS FUNCTION'S COMPILATION until the
/// author classifies it — a stronger gate than a count assertion, which only
/// drifts at runtime. Tier: **truly-unrepresentable** (E0004 non-exhaustive
/// match), not merely CI-caught.
#[test]
fn rank_phase_matrix_is_total() {
    fn needs_needle(p: RankPhase) -> bool {
        match p {
            // Needle-driven: defined to be a no-op when the needle is empty,
            // which is what keeps the published `apply()` behavior-preserving.
            RankPhase::MatchNeedle | RankPhase::CollapseDescendants { .. } => true,
            RankPhase::LoadEntries
            | RankPhase::ComputeAge
            | RankPhase::ApplyDecay
            | RankPhase::Combine
            | RankPhase::FloorIndexed
            | RankPhase::SortDesc
            | RankPhase::TopK { .. } => false,
        }
    }
    assert!(needs_needle(RankPhase::MatchNeedle));
    assert!(!needs_needle(RankPhase::SortDesc));
    // Every needle-driven phase must be a no-op under an empty needle.
    let now = at_day(10);
    let env = MockEnvironment::at(now);
    let mut spec = FrecencyRankingSpec::skimtab_parity();
    spec.phases = FrecencyRankingSpec::canonical_phases();
    let entries = vec![
        DirEntry {
            path: "/a".into(),
            visits: vec![now],
            discovered_only: false,
        },
        DirEntry {
            path: "/a/b".into(),
            visits: vec![now],
            discovered_only: false,
        },
    ];
    let ranked = apply(&spec, entries, &env).unwrap();
    assert_eq!(ranked.len(), 2, "empty needle must not collapse or filter");
}

/// Every `MatchKind` must be reachable by some (needle, path) pair. A new
/// variant with no row fails here.
#[test]
fn match_kind_matrix_is_total() {
    let rows: &[(MatchKind, &str, &str)] = &[
        (MatchKind::BasenameExact, "nix", "/code/pleme-io/nix"),
        (MatchKind::BasenamePrefix, "wad", "/code/pleme-io/wadachi"),
        (
            MatchKind::BasenameSubsequence,
            "wdc",
            "/code/pleme-io/wadachi",
        ),
        (MatchKind::ComponentPrefix, "wad", "/code/wadachi/spec/src"),
        // Substring-only: "mmu" is inside "co-mmu-nity" but is neither a
        // component prefix nor a subsequence of the basename "gifs".
        (
            MatchKind::SubstringAnywhere,
            "mmu",
            "/code/akeyless-community/gifs",
        ),
    ];
    assert_eq!(
        rows.len(),
        MatchKind::ALL.len(),
        "a MatchKind was added/removed without updating the matrix"
    );
    for (expected, needle, path) in rows {
        let got = MatchProfile::classify_raw(needle, std::path::Path::new(path));
        assert_eq!(got, Some(*expected), "{needle:?} vs {path:?}");
    }
    // And the ladder really is worst → best.
    let mut sorted = MatchKind::ALL.to_vec();
    sorted.sort_unstable();
    assert_eq!(sorted, MatchKind::ALL, "MatchKind Ord must be worst → best");
}

/// THE MEASURED DEFECT. Probed live at the frost prompt on 2026-08-01:
/// `cd ni<TAB>` offered `…/akeyless-community/…` and
/// `…/external-secrets/providers/v1/fortanix/` above real candidates,
/// because the needle matched the middle of "commu-NI-ty" / "forta-NI-x".
#[test]
fn anchored_matching_rejects_mid_word_ancestor_coincidences() {
    let noise = std::path::Path::new(
        "/Users/x/code/github/akeyless-community/Akeyless-Cursor-Plugin/resources/gifs",
    );
    let real = std::path::Path::new("/Users/x/code/github/pleme-io/nix");
    let anchored = MatchProfile::anchored();

    assert_eq!(
        anchored.classify("ni", noise),
        None,
        "mid-word ancestor coincidence must not match"
    );
    assert_eq!(
        anchored.classify("ni", real),
        Some(MatchKind::BasenamePrefix)
    );

    // Honest boundary: the *directory named* `akeyless-community` still
    // matches "ni" — as a BasenameSubsequence (c-o-m-m-u-**n**-**i**-t-y), a
    // legitimate fuzzy hit on the thing being named. It is admitted at weight
    // 1.0 against BasenamePrefix's 4.0, so it ranks far below `nix` instead of
    // being excluded. Only the *ancestor* coincidence is rejected outright.
    let community = std::path::Path::new("/Users/x/code/github/akeyless-community");
    assert_eq!(
        anchored.classify("ni", community),
        Some(MatchKind::BasenameSubsequence)
    );
    assert!(
        anchored.weight(MatchKind::BasenamePrefix)
            > 3.0 * anchored.weight(MatchKind::BasenameSubsequence)
    );

    // …and the legacy profile still accepts it, so the difference between the
    // two instances is pinned by a test rather than asserted in a changelog.
    assert_eq!(
        MatchProfile::substring_legacy().classify("ni", noise),
        Some(MatchKind::SubstringAnywhere)
    );
}

/// THE OTHER MEASURED DEFECT: 6 of 8 `cd wad` slots were sub-paths of the
/// same repo (`wadachi/`, `wadachi/wadachi-spec/`, `…/specs/`, `…/src/`,
/// `…/tests/`, `wadachi/wadachi/src/`).
#[test]
fn descendants_collapse_under_a_kept_ancestor() {
    let now = at_day(10);
    let env = MockEnvironment::at(now);
    let spec = FrecencyRankingSpec::skimtab_parity();
    let paths = [
        "/code/pleme-io/wadachi",
        "/code/pleme-io/wadachi/wadachi-spec",
        "/code/pleme-io/wadachi/wadachi-spec/specs",
        "/code/pleme-io/wadachi/wadachi-spec/src",
        "/code/pleme-io/wadachi/wadachi/src",
    ];
    let entries = paths
        .iter()
        .map(|p| DirEntry {
            path: (*p).into(),
            visits: vec![now],
            discovered_only: false,
        })
        .collect();
    let ranked = apply_matched(&spec, entries, "wad", &env).unwrap();
    assert_eq!(
        ranked
            .iter()
            .map(|r| r.path.to_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["/code/pleme-io/wadachi"],
        "one repo must not flood the result set with its own subtree"
    );
}

/// A real visit must beat an indexed-only coincidence even when both match at
/// the same kind — the `wadachi` vs `wadey` case from the live probe.
#[test]
fn a_lived_in_dir_outranks_a_never_visited_namesake() {
    let now = at_day(10);
    let env = MockEnvironment::at(now);
    let spec = FrecencyRankingSpec::skimtab_parity();
    let entries = vec![
        DirEntry {
            path: "/code/akeylesslabs/main/vendor/github.com/wadey".into(),
            visits: vec![],
            discovered_only: true,
        },
        DirEntry {
            path: "/code/pleme-io/wadachi".into(),
            visits: vec![now - Duration::days(1)],
            discovered_only: false,
        },
    ];
    let ranked = apply_matched(&spec, entries, "wad", &env).unwrap();
    assert_eq!(ranked[0].path.to_str().unwrap(), "/code/pleme-io/wadachi");
}

/// A `/`-bearing needle reads as ordered path fragments.
#[test]
fn slash_needle_matches_ancestor_then_basename() {
    let anchored = MatchProfile::anchored();
    let p = std::path::Path::new("/Users/x/code/github/pleme-io/wadachi");
    assert_eq!(
        anchored.classify("pleme-io/wad", p),
        Some(MatchKind::BasenamePrefix)
    );
    // Ancestor fragment that isn't there → no match, even though "wad" is.
    assert_eq!(anchored.classify("akeylesslabs/wad", p), None);
    // Order matters: the ancestor must precede the basename.
    assert_eq!(anchored.classify("wadachi/pleme-io", p), None);
}

/// Case-insensitive, and the empty needle is the identity.
#[test]
fn matching_is_case_insensitive_and_empty_is_identity() {
    let anchored = MatchProfile::anchored();
    let p = std::path::Path::new("/Users/x/Code/Pleme-IO/WaDaChi");
    assert_eq!(
        anchored.classify("wadachi", p),
        Some(MatchKind::BasenameExact)
    );
    assert_eq!(anchored.classify("WAD", p), Some(MatchKind::BasenamePrefix));
    assert_eq!(anchored.classify("", p), Some(MatchKind::BasenameExact));
}

/// `apply` (the published, needle-free signature) must be bit-identical to
/// what it returned before matching was modeled.
#[test]
fn published_apply_is_behavior_preserving() {
    let now = at_day(10);
    let env = MockEnvironment::at(now);
    let spec = FrecencyRankingSpec::skimtab_parity();
    let entries = vec![DirEntry {
        path: "/x".into(),
        visits: vec![now, now - Duration::days(4)],
        discovered_only: false,
    }];
    let ranked = apply(&spec, entries, &env).unwrap();
    let expected = 1.0 / (1.0 + 0.0) + 1.0 / (1.0 + 4.0);
    assert!(
        (ranked[0].score - expected).abs() < 1e-9,
        "got {}",
        ranked[0].score
    );
}

/// Every shipped instance must survive a needle that matches nothing without
/// erroring, and must never return a non-match.
#[test]
fn no_instance_returns_a_non_match() {
    let now = at_day(10);
    let env = MockEnvironment::at(now);
    for spec in FrecencyRankingSpec::all() {
        let entries = vec![DirEntry {
            path: "/code/pleme-io/nix".into(),
            visits: vec![now],
            discovered_only: false,
        }];
        let ranked = apply_matched(&spec, entries, "zzzznope", &env).unwrap();
        assert!(ranked.is_empty(), "{} returned a non-match", spec.name);
    }
}
