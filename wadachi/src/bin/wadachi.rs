//! `wadachi` CLI — the operator + integration surface (a thin client over the
//! in-process facade; consumers like frost link the library directly).
//!
//! - `wadachi add [PATH]`         record a visit (default: cwd)
//! - `wadachi query [NEEDLE]`     ranked "score<TAB>path"
//! - `wadachi resolve NEEDLE`     the single best path (exit 1 if none) — smart-cd
//! - `wadachi index`              one-shot full indexer pass (walk + upsert + prune)
//! - `wadachi indexd`             ashiato-niwa daemon: initial walk + notify watch loop
//! - `wadachi config-show [TIER]` effective config (bare | discovered | default)

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use pleme_io_wadachi::config::WadachiConfig;
use pleme_io_wadachi::{DirFrecencyDb, indexer};

#[derive(Parser)]
#[command(
    name = "wadachi",
    version,
    about = "directory frecency — the well-worn rut (轍)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Record a visit to PATH (default: current directory).
    Add {
        /// Directory to record. Defaults to cwd.
        path: Option<String>,
    },
    /// List directories matching NEEDLE, ranked by frecency.
    Query {
        /// Case-insensitive substring; empty matches all.
        needle: Option<String>,
        /// Max results.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Print the single best match for NEEDLE (exit 1 if none). For smart-cd.
    Resolve {
        /// Case-insensitive substring.
        needle: String,
    },
    /// One-shot full indexer pass over the configured roots (upsert + prune).
    Index,
    /// Run the ashiato-niwa background indexer daemon: one initial full walk,
    /// then a notify watcher plus gate-checked periodic re-walks.
    Indexd,
    /// Import zoxide's database (history kept when zoxide is configured off).
    ImportZoxide {
        /// Path to db.zo. Default: zoxide's own location.
        #[arg(long)]
        db: Option<std::path::PathBuf>,
        /// Rank the import in memory and print "score path" (zoxide's
        /// `query -ls` shape) instead of writing the store.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the zsh/bash hook for shells that are not frost. The hook never
    /// writes to the shell's stdout or stderr.
    Init {
        #[arg(value_enum)]
        shell: InitShell,
    },
    /// Show the effective configuration for a tier (default: the active tier).
    ConfigShow {
        /// `bare` | `discovered` | `default`. Omit for the WADACHI_TIER-active tier.
        tier: Option<String>,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum InitShell {
    Zsh,
    Bash,
}

const HOOK_COMMON: &str = r#"z() {
  if [ "$#" -eq 0 ]; then builtin cd || return; return; fi
  if [ "$#" -eq 1 ] && { [ -d "$1" ] || [ "$1" = - ]; }; then builtin cd -- "$1"; return; fi
  __wadachi_t=$(command wadachi resolve "$*" 2>/dev/null) || { unset __wadachi_t; return 1; }
  builtin cd -- "$__wadachi_t"; __wadachi_rc=$?; unset __wadachi_t; return $__wadachi_rc
}
"#;

const HOOK_ZSH: &str = r#"__wadachi_hook() { command wadachi add -- "$PWD" >/dev/null 2>&1 &! }
autoload -Uz add-zsh-hook
add-zsh-hook chpwd __wadachi_hook
"#;

const HOOK_BASH: &str = r#"__wadachi_hook() {
  [ "$__wadachi_pwd" = "$PWD" ] && return
  __wadachi_pwd=$PWD
  (command wadachi add -- "$PWD" >/dev/null 2>&1 &)
}
case ";${PROMPT_COMMAND:-};" in *";__wadachi_hook;"*) ;; *) PROMPT_COMMAND="__wadachi_hook;${PROMPT_COMMAND:-}" ;; esac
"#;

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Add { path } => {
            let p = match path {
                Some(p) => p,
                None => std::env::current_dir()
                    .context("resolving cwd")?
                    .to_string_lossy()
                    .into_owned(),
            };
            pleme_io_wadachi::record(&p)?;
        }
        Cmd::Query { needle, limit } => {
            for r in pleme_io_wadachi::top_n(&needle.unwrap_or_default(), limit)? {
                println!("{:.4}\t{}", r.score, r.path.display());
            }
        }
        Cmd::Resolve { needle } => match pleme_io_wadachi::resolve(&needle)? {
            Some(p) => println!("{}", p.display()),
            None => std::process::exit(1),
        },
        Cmd::Index => {
            let cfg = WadachiConfig::active();
            let store = DirFrecencyDb::open(&cfg.db_path)?;
            let summary = indexer::index_once(&store, &cfg.indexer)?;
            println!("{summary}");
        }
        Cmd::Indexd => {
            let cfg = WadachiConfig::active();
            let store = DirFrecencyDb::open(&cfg.db_path)?;
            indexer::run_daemon(&store, &cfg.indexer, |pass| println!("{pass}"))?;
        }
        Cmd::ImportZoxide { db, dry_run } => {
            use pleme_io_wadachi::wadachi_spec::{FrecencyRankingSpec, RankPhase};
            use pleme_io_wadachi::{query, store::MemDirStore, zoxide};
            let db = db
                .or_else(zoxide::default_db_path)
                .context("no zoxide db path: pass --db")?;
            let dirs = zoxide::read(&db).with_context(|| db.display().to_string())?;
            if dry_run {
                let mem = MemDirStore::new();
                zoxide::import(&mem, &dirs)?;
                let mut spec = FrecencyRankingSpec::praca_parity();
                spec.phases.retain(|p| !matches!(p, RankPhase::TopK { .. }));
                for r in query::top_n(&mem, &spec, "", usize::MAX)? {
                    println!("{:>6.1} {}", r.score, r.path.display());
                }
            } else {
                let store = DirFrecencyDb::open(&pleme_io_wadachi::runtime_db_path())?;
                println!("{}", zoxide::import(&store, &dirs)?);
            }
        }
        Cmd::Init { shell } => {
            print!("{HOOK_COMMON}");
            print!(
                "{}",
                match shell {
                    InitShell::Zsh => HOOK_ZSH,
                    InitShell::Bash => HOOK_BASH,
                }
            );
        }
        Cmd::ConfigShow { tier } => {
            let cfg = match tier.as_deref() {
                Some("bare") => WadachiConfig::bare(),
                Some("discovered") => WadachiConfig::discovered(),
                Some("default") => WadachiConfig::prescribed_default(),
                _ => WadachiConfig::active(),
            };
            print_config(&cfg);
        }
    }
    Ok(())
}

fn print_config(c: &WadachiConfig) {
    println!("db_path                       {}", c.db_path.display());
    println!("ranking_instance              {}", c.ranking_instance);
    println!("indexer.enabled               {}", c.indexer.enabled);
    println!(
        "indexer.roots                 {} dirs",
        c.indexer.roots.len()
    );
    for r in &c.indexer.roots {
        println!(
            "                                {} (depth {})",
            r.path.display(),
            r.max_depth
        );
    }
    println!(
        "indexer.ignore_names          {}",
        c.indexer.ignore_names.join(" ")
    );
    println!("indexer.index_hidden          {}", c.indexer.index_hidden);
    println!("indexer.debounce_ms           {}", c.indexer.debounce_ms);
    println!(
        "indexer.rewalk_interval_secs  {}",
        c.indexer.rewalk_interval_secs
    );
    println!("indexer.concurrency           {}", c.indexer.concurrency);
    println!("max_entries                   {}", c.max_entries);
    println!("cleanup_max_age_days          {}", c.cleanup_max_age_days);
}
