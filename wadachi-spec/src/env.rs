//! The interpreter's one side effect — the clock — behind a trait so tests
//! are deterministic with no real time, no DB, no flake.

use chrono::{NaiveDateTime, Utc};

/// The only environmental capability the frecency interpreter needs: the
/// current time, used to compute each visit's age. Real implementations read
/// the wall clock; tests pin it.
pub trait FrecencyEnvironment {
    /// "Now", as a naive UTC timestamp.
    fn now(&self) -> NaiveDateTime;
}

/// Production environment — reads the wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealEnvironment;

impl FrecencyEnvironment for RealEnvironment {
    fn now(&self) -> NaiveDateTime {
        Utc::now().naive_utc()
    }
}

/// Test environment — a frozen clock so ranking is fully deterministic.
#[derive(Debug, Clone, Copy)]
pub struct MockEnvironment {
    /// The fixed "now" every call returns.
    pub fixed: NaiveDateTime,
}

impl MockEnvironment {
    /// Freeze the clock at `fixed`.
    #[must_use]
    pub fn at(fixed: NaiveDateTime) -> Self {
        Self { fixed }
    }
}

impl FrecencyEnvironment for MockEnvironment {
    fn now(&self) -> NaiveDateTime {
        self.fixed
    }
}
