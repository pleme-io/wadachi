# wadachi-spec (轍)

The **zero-I/O frecency-ranking core** shared across the pleme-io fleet — one
formula for directories (`wadachi`), command history (skim-tab), and zoxide
imports, so they cannot drift.

Authored as the pleme-io TYPED-SPEC + INTERPRETER TRIPLET:

- **Typed border** — `FrecencyRankingSpec` + `DecayKind` + `RankPhase`.
- **Authored Lisp spec** — [`specs/frecency.lisp`](./specs/frecency.lisp) declares
  the canonical instances (`skimtab-parity`, `zoxide-parity`) as data.
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
behavior-preserving extraction. MIT.
