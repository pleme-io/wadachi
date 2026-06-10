//! Typed configuration for wadachi.
//!
//! > **Fast-follow.** This is a plain typed struct today; the destination is a
//! > full `shikumi::TieredConfig` impl (bare / discovered / `prescribed_default`
//! > / extend / diff + `WADACHI_TIER` env + `wadachi config-show <tier>`),
//! > matching the fleet configuration prime directive. Tracked in CLAUDE.md.

use std::path::PathBuf;

/// Where wadachi keeps its state and how it ranks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WadachiConfig {
    /// Path to the SQLite frecency database (the shared inter-process bus).
    pub db_path: PathBuf,
    /// Name of the [`wadachi_spec::FrecencyRankingSpec`] instance to rank with.
    pub ranking_instance: String,
}

impl Default for WadachiConfig {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
            ranking_instance: "skimtab-parity".to_owned(),
        }
    }
}

impl WadachiConfig {
    /// Resolve config, honoring `WADACHI_DB` and `WADACHI_RANKING` overrides.
    #[must_use]
    pub fn resolve() -> Self {
        let mut c = Self::default();
        if let Ok(p) = std::env::var("WADACHI_DB") {
            c.db_path = PathBuf::from(p);
        }
        if let Ok(r) = std::env::var("WADACHI_RANKING") {
            c.ranking_instance = r;
        }
        c
    }

    /// The ranking spec named by [`Self::ranking_instance`], falling back to
    /// the fleet default if the name is unknown.
    #[must_use]
    pub fn spec(&self) -> wadachi_spec::FrecencyRankingSpec {
        wadachi_spec::FrecencyRankingSpec::by_name(&self.ranking_instance)
            .unwrap_or_else(wadachi_spec::FrecencyRankingSpec::skimtab_parity)
    }
}

/// `$XDG_DATA_HOME/wadachi/dirs.db` (or the platform data dir), e.g.
/// `~/.local/share/wadachi/dirs.db`.
#[must_use]
pub fn default_db_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("wadachi").join("dirs.db")
}
