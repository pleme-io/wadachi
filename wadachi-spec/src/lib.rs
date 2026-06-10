//! `wadachi-spec` (轍) — the frecency-ranking core, shared fleet-wide.
//!
//! This crate is **zero-I/O on purpose**: it owns exactly one thing — *how a
//! path's worn-ness is scored from when it was visited* — and nothing else
//! (no SQLite, no filesystem, no clock except behind a trait). That lets every
//! consumer depend on it without dragging in storage: `wadachi`'s directory
//! store, skim-tab's command history, and a zoxide import all rank through
//! [`apply`] so there is exactly one formula and they cannot drift.
//!
//! It is authored as the pleme-io TYPED-SPEC + INTERPRETER TRIPLET:
//! - **Typed border** — [`FrecencyRankingSpec`] + [`DecayKind`] + [`RankPhase`]
//!   ([`spec`]).
//! - **Authored Lisp spec** — `specs/frecency.lisp` declares the canonical
//!   instances as data.
//! - **Interpreter** — [`apply`] walks the phases against a mockable
//!   [`FrecencyEnvironment`] ([`interp`], [`env`]).
//!
//! ```
//! use wadachi_spec::{apply, FrecencyRankingSpec, DirEntry, MockEnvironment};
//! use chrono::NaiveDate;
//!
//! let now = NaiveDate::from_ymd_opt(2026, 6, 9).unwrap().and_hms_opt(0, 0, 0).unwrap();
//! let env = MockEnvironment::at(now);
//! let spec = FrecencyRankingSpec::skimtab_parity();
//! let entries = vec![DirEntry {
//!     path: "/code".into(),
//!     visits: vec![now], // visited "now" → age 0 → score 1.0
//!     discovered_only: false,
//! }];
//! let ranked = apply(&spec, entries, &env).unwrap();
//! assert!((ranked[0].score - 1.0).abs() < 1e-9);
//! ```

pub mod env;
pub mod interp;
pub mod spec;

pub use env::{FrecencyEnvironment, MockEnvironment, RealEnvironment};
pub use interp::{apply, SpecError};
pub use spec::{DecayKind, DirEntry, FrecencyRankingSpec, RankPhase, RankedDir};
