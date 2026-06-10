//! `wadachi` CLI — the operator + integration surface.
//!
//! - `wadachi add [PATH]`      record a visit (default: cwd) — the chpwd hook
//! - `wadachi query [NEEDLE]`  ranked list "score<TAB>path"
//! - `wadachi resolve NEEDLE`  the single best path (exit 1 if none) — smart-cd
//! - `wadachi config-show`     effective config

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use wadachi::{config::WadachiConfig, query, store::DirFrecencyDb, store::DirStore};

#[derive(Parser)]
#[command(name = "wadachi", version, about = "directory frecency — the well-worn rut (轍)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Record a visit to PATH (default: current directory).
    Add {
        /// Directory to record (absolute recommended). Defaults to cwd.
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
    /// Show the effective configuration.
    ConfigShow,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = WadachiConfig::resolve();

    match cli.cmd {
        Cmd::Add { path } => {
            let p = match path {
                Some(p) => p,
                None => std::env::current_dir()
                    .context("resolving cwd")?
                    .to_string_lossy()
                    .into_owned(),
            };
            let store = DirFrecencyDb::open(&cfg.db_path)?;
            store.record(&p)?;
        }
        Cmd::Query { needle, limit } => {
            let store = DirFrecencyDb::open(&cfg.db_path)?;
            let needle = needle.unwrap_or_default();
            for r in query::top_n(&store, &cfg.spec(), &needle, limit)? {
                println!("{:.4}\t{}", r.score, r.path.display());
            }
        }
        Cmd::Resolve { needle } => {
            let store = DirFrecencyDb::open(&cfg.db_path)?;
            match query::top_match(&store, &cfg.spec(), &needle)? {
                Some(p) => println!("{}", p.display()),
                None => std::process::exit(1),
            }
        }
        Cmd::ConfigShow => {
            println!("db_path          {}", cfg.db_path.display());
            println!("ranking_instance {}", cfg.ranking_instance);
        }
    }
    Ok(())
}
