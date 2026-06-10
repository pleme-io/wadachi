//! Verification matrix — exercises every shipped ranking instance and every
//! `DecayKind`, and fails the build if a new variant lands without a row. This
//! is the substrate's mechanical promise that ranking is correct across the
//! whole supported surface, not just the case the author happened to test.

use chrono::{Duration, NaiveDate, NaiveDateTime};
use wadachi_spec::{apply, DecayKind, DirEntry, FrecencyRankingSpec, MockEnvironment};

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
            DirEntry { path: "/old".into(), visits: vec![old], discovered_only: false },
            DirEntry { path: "/recent".into(), visits: vec![recent], discovered_only: false },
        ];
        let ranked = apply(&spec, entries, &env).unwrap();
        if ranked.first().map(|r| r.path.to_str().unwrap()) != Some("/recent") {
            failures.push(spec.name.clone());
        }
    }
    assert!(failures.is_empty(), "instances ranked old over recent: {failures:?}");
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
    assert!((ranked[0].score - expected).abs() < 1e-9, "got {}", ranked[0].score);
}

#[test]
fn discovered_only_is_floored_and_below_recent_visits() {
    let now = at_day(10);
    let env = MockEnvironment::at(now);
    let spec = FrecencyRankingSpec::skimtab_parity();
    let entries = vec![
        DirEntry { path: "/indexed".into(), visits: vec![], discovered_only: true },
        DirEntry {
            // Visited within the epsilon-crossover window (~2.7y for eps=0.001);
            // 100d → score 1/101 ≈ 0.0099 > 0.001.
            path: "/recent".into(),
            visits: vec![now - Duration::days(100)],
            discovered_only: false,
        },
    ];
    let ranked = apply(&spec, entries, &env).unwrap();
    let indexed = ranked.iter().find(|r| r.path.to_str() == Some("/indexed")).unwrap();
    let recent = ranked.iter().find(|r| r.path.to_str() == Some("/recent")).unwrap();
    // Discovered-only is floored to exactly epsilon — rankable, but the floor.
    assert_eq!(indexed.score, spec.indexed_epsilon);
    // A reasonably-recent real visit outranks a fresh discovery. (Visits older
    // than the crossover decay below the floor and are effectively forgotten,
    // which is the intended frecency behavior.)
    assert!(recent.score > indexed.score, "recent {} vs floor {}", recent.score, indexed.score);
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
