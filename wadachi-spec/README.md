# wadachi-spec (轍)

The **zero-I/O frecency-ranking core** shared across the pleme-io fleet — one
formula for directories (`wadachi`), command history (skim-tab), and zoxide
imports, so they cannot drift.

Authored as the pleme-io TYPED-SPEC + INTERPRETER TRIPLET:

- **Typed border** — `FrecencyRankingSpec` + `DecayKind` + `CombineKind` +
  `RankPhase`.
- **Authored Lisp spec** — [`specs/frecency.lisp`](./specs/frecency.lisp) declares
  the canonical instances (`skimtab-parity`, `zoxide-parity`, `praca-parity`,
  `substring-legacy`) as data.
- **Interpreter** — `apply(spec, entries, env)` walks the phases against a
  mockable `FrecencyEnvironment` (the clock is the only side effect → tests are
  deterministic).

```rust
use wadachi_spec::{apply, FrecencyRankingSpec, DirEntry, MockEnvironment};
use chrono::NaiveDate;

let now = NaiveDate::from_ymd_opt(2026, 6, 9).unwrap().and_hms_opt(0, 0, 0).unwrap();
let ranked = apply(
    &FrecencyRankingSpec::skimtab_parity(),
    vec![DirEntry { path: "/code".into(), visits: vec![now], discovered_only: false }],
    &MockEnvironment::at(now),
).unwrap();
assert!((ranked[0].score - 1.0).abs() < 1e-9);
```

`skimtab-parity` is `Σ 1/(1+age_days)` — behavior-identical to skim-tab's
historical `frecency_score`, which makes adopting this crate in skim-tab a
behavior-preserving extraction.

## Combines, and consumers that don't keep a visit log

How the decayed weights and the visit count fold into one score is a named
`CombineKind`, not a fixed expression:

| `CombineKind` | score |
|---|---|
| `RecencySumPlusFreq` (default) | `recency_weight · Σ decay(age_i) + freq_weight · n` |
| `FreqTimesLatestDecay` | `recency_weight · n · decay(age of the last visit)` |

`praca-parity` selects the second over `ZoxideLogBuckets` — which is what zoxide
actually computes, and what tear's praça session ranking has always computed.

A consumer that stores a **visit counter plus one timestamp** rather than a
per-visit log has no `Σ` to take; `FrecencyRankingSpec::score_counted(visits,
age_days)` is that projection, and it goes through the same `combine_score` the
interpreter's `Combine` phase does. Reach for it instead of re-deriving the
curve — that re-derivation is exactly how `tear/praca/src/frecency.rs` ended up
holding a byte-identical copy of the `ZoxideLogBuckets` thresholds. MIT.
