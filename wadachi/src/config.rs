//! Typed, **tiered** configuration — the discoverable-defaults surface.
//!
//! Mirrors `shikumi::TieredConfig`'s `bare` / `discovered` / `prescribed_default`
//! shape so every consumer (frost, mado, skim-cd) converges on the **same store
//! with zero config** — the cross-tool "it just works together" guarantee comes
//! from them all resolving the same discovered `db_path`.
//!
//! Implemented natively rather than via the `shikumi::TieredConfig` trait so
//! wadachi stays crates.io-publishable (shikumi's crates.io release predates the
//! trait, and a git dep would block publishing). The method shapes match the
//! trait, so adopting it later — once shikumi publishes a current version — is
//! mechanical.
//!
//! Hot-path note: the per-`cd` facade ([`crate::record`] / [`crate::resolve`])
//! does **not** build a full config or touch the filesystem-walking
//! `discovered()` — it uses the cheap [`runtime_db_path`] + ranking only. The
//! full tiered config is for `config-show` and the background indexer.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which configuration tier to materialize. Selected by `WADACHI_TIER`
/// (`bare` | `discovered` | `default`) — the fleet `<APP>_TIER` convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigTier {
    /// The documented floor — zero opinions, indexer off.
    Bare,
    /// `bare` + runtime autodetect (XDG paths, workspace roots, nproc).
    Discovered,
    /// `discovered` + curated defaults — the prescribed first-run experience.
    Default,
}

impl ConfigTier {
    /// Resolve the tier from `WADACHI_TIER` (defaults to [`ConfigTier::Default`]).
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("WADACHI_TIER").ok().as_deref() {
            Some("bare") => Self::Bare,
            Some("discovered") => Self::Discovered,
            _ => Self::Default,
        }
    }
}

/// Where wadachi keeps its state and how it ranks / indexes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WadachiConfig {
    /// Path to the SQLite frecency database — the shared inter-process bus.
    pub db_path: PathBuf,
    /// Name of the [`wadachi_spec::FrecencyRankingSpec`] instance to rank with.
    pub ranking_instance: String,
    /// Whether the aggressive background indexer runs.
    pub indexer_enabled: bool,
    /// Directory trees the indexer walks to surface never-visited jump targets.
    pub indexer_roots: Vec<PathBuf>,
    /// Directory names the indexer skips (also honors per-repo `.gitignore`).
    pub ignore_globs: Vec<String>,
    /// Max walk depth (0 = unbounded only at `bare`).
    pub max_depth: usize,
    /// Seconds between periodic re-index passes.
    pub indexer_interval_secs: u64,
    /// Bounded concurrent walks.
    pub indexer_concurrency: usize,
    /// Cap on stored entries (0 = uncapped).
    pub max_entries: usize,
    /// Visits older than this are GC-eligible (0 = never).
    pub cleanup_max_age_days: f64,
}

impl WadachiConfig {
    /// Tier 0 — the documented floor. The db lives at the standard path (so the
    /// shared bus still works), ranking is the fleet default, everything else
    /// is off / empty.
    #[must_use]
    pub fn bare() -> Self {
        Self {
            db_path: default_db_path(),
            ranking_instance: "skimtab-parity".to_owned(),
            indexer_enabled: false,
            indexer_roots: Vec::new(),
            ignore_globs: Vec::new(),
            max_depth: 0,
            indexer_interval_secs: 0,
            indexer_concurrency: 1,
            max_entries: 0,
            cleanup_max_age_days: 0.0,
        }
    }

    /// Tier 1 — `bare` + runtime autodetect: the db honors `$XDG_DATA_HOME`,
    /// indexer roots are the `~/code/${service}/${org}` workspace dirs, and
    /// concurrency tracks the core count.
    #[must_use]
    pub fn discovered() -> Self {
        let mut c = Self::bare();
        c.db_path = default_db_path();
        c.indexer_roots = discover_workspace_roots();
        c.indexer_concurrency = std::thread::available_parallelism().map_or(1, |n| n.get());
        c
    }

    /// Tier 2 — `discovered` + curated defaults: the indexer is on, `~` and
    /// `~/code` are added to the roots, the standard ignore set + bounds apply.
    #[must_use]
    pub fn prescribed_default() -> Self {
        let mut c = Self::discovered();
        c.indexer_enabled = true;
        if let Some(home) = dirs::home_dir() {
            c.indexer_roots.push(home.join("code"));
            c.indexer_roots.push(home);
        }
        c.ignore_globs = [
            ".git", "node_modules", "target", "__pycache__", ".direnv", "result",
            ".cargo", ".rustup",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        c.max_depth = 8;
        c.indexer_interval_secs = 900;
        c.indexer_concurrency = c.indexer_concurrency.max(8);
        c.max_entries = 50_000;
        c.cleanup_max_age_days = 180.0;
        c
    }

    /// Materialize the tier (the operator entry point for tooling/indexer).
    #[must_use]
    pub fn resolve_tier(tier: ConfigTier) -> Self {
        match tier {
            ConfigTier::Bare => Self::bare(),
            ConfigTier::Discovered => Self::discovered(),
            ConfigTier::Default => Self::prescribed_default(),
        }
    }

    /// The active config: the `WADACHI_TIER` tier with `WADACHI_DB` /
    /// `WADACHI_RANKING` point overlays. Used by `config-show` and the indexer
    /// (NOT the per-`cd` hot path — see [`crate::record`]).
    #[must_use]
    pub fn active() -> Self {
        let mut c = Self::resolve_tier(ConfigTier::from_env());
        if let Ok(p) = std::env::var("WADACHI_DB") {
            c.db_path = PathBuf::from(p);
        }
        if let Ok(r) = std::env::var("WADACHI_RANKING") {
            c.ranking_instance = r;
        }
        c
    }

    /// The ranking spec named by [`Self::ranking_instance`], falling back to the
    /// fleet default for an unknown name.
    #[must_use]
    pub fn spec(&self) -> wadachi_spec::FrecencyRankingSpec {
        wadachi_spec::FrecencyRankingSpec::by_name(&self.ranking_instance)
            .unwrap_or_else(wadachi_spec::FrecencyRankingSpec::skimtab_parity)
    }
}

impl Default for WadachiConfig {
    fn default() -> Self {
        Self::prescribed_default()
    }
}

/// `$XDG_DATA_HOME/wadachi/dirs.db` (or the platform data dir), e.g.
/// `~/.local/share/wadachi/dirs.db`. The single well-known path every consumer
/// resolves to — that shared default is what makes them work together.
#[must_use]
pub fn default_db_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("wadachi").join("dirs.db")
}

/// The `~/code/${service}/${org}` directories under `~/code` — the pleme-io
/// workspace convention the indexer walks to surface repos as jump targets.
fn discover_workspace_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return roots;
    };
    let code = home.join("code");
    let Ok(services) = std::fs::read_dir(&code) else {
        return roots;
    };
    for service in services.flatten() {
        let sp = service.path();
        if !sp.is_dir() {
            continue;
        }
        if let Ok(orgs) = std::fs::read_dir(&sp) {
            for org in orgs.flatten() {
                let op = org.path();
                if op.is_dir() {
                    roots.push(op);
                }
            }
        }
    }
    roots
}
