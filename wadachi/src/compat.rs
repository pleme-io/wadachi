//! `wadachi zoxide …` — zoxide's CLI surface over the wadachi store, so a
//! `zoxide` alias keeps scripts and muscle memory working.
//!
//! Supported: `add`, `query [-l] [-s] [-a] [--exclude P] [KEYWORDS…]`,
//! `remove`, `import`, `init`, `--version`. `query -i` and `edit` are refused
//! with exit 2 and point at `zi`.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::store::DirStore;
use crate::wadachi_spec::FrecencyRankingSpec;
use crate::{hook, query};

fn absolute(p: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(p);
    let joined = if p.is_absolute() {
        p.to_owned()
    } else {
        cwd.join(p)
    };
    joined.canonicalize().unwrap_or(joined)
}

fn matches_all(path: &Path, keywords: &[String]) -> bool {
    let hay = path.to_string_lossy().to_lowercase();
    let mut from = 0;
    for k in keywords {
        match hay[from..].find(&k.to_lowercase()) {
            Some(i) => from += i + k.len(),
            None => return false,
        }
    }
    keywords.last().is_none_or(|last| {
        path.file_name().is_some_and(|n| {
            n.to_string_lossy()
                .to_lowercase()
                .contains(&last.to_lowercase())
        })
    })
}

/// Run one zoxide-shaped invocation. Returns the exit code.
///
/// # Errors
/// Propagates store and I/O failures.
pub fn run(
    store: &dyn DirStore,
    spec: &FrecencyRankingSpec,
    args: &[String],
    cwd: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<i32> {
    let Some((sub, rest)) = args.split_first() else {
        writeln!(err, "usage: zoxide <add|query|remove|import|init> …")?;
        return Ok(2);
    };
    match sub.as_str() {
        "-V" | "--version" => {
            writeln!(out, "zoxide (wadachi {})", env!("CARGO_PKG_VERSION"))?;
            Ok(0)
        }
        "add" => {
            for p in rest.iter().filter(|a| a.as_str() != "--") {
                store.record(&absolute(p, cwd).to_string_lossy())?;
            }
            Ok(0)
        }
        "remove" | "rm" => {
            for p in rest.iter().filter(|a| a.as_str() != "--") {
                store.forget(&absolute(p, cwd).to_string_lossy())?;
            }
            Ok(0)
        }
        "init" => {
            let shell = match rest.first().map(String::as_str) {
                Some("zsh") => hook::Shell::Zsh,
                Some("bash") => hook::Shell::Bash,
                _ => {
                    writeln!(err, "zoxide init: only zsh and bash are supported")?;
                    return Ok(2);
                }
            };
            let cmd = rest
                .iter()
                .position(|a| a == "--cmd")
                .and_then(|i| rest.get(i + 1))
                .map_or(Ok(None), |c| c.parse::<hook::CmdName>().map(Some));
            match cmd {
                Ok(cmd) => {
                    write!(out, "{}", hook::render(shell, cmd.as_ref()))?;
                    Ok(0)
                }
                Err(e) => {
                    writeln!(err, "zoxide init --cmd: {e}")?;
                    Ok(2)
                }
            }
        }
        "import" => {
            let Some(path) = rest.iter().rfind(|a| !a.starts_with('-')) else {
                writeln!(err, "zoxide import: pass the db path")?;
                return Ok(2);
            };
            let dirs = crate::zoxide::read(Path::new(path))?;
            writeln!(out, "{}", crate::zoxide::import(store, &dirs)?)?;
            Ok(0)
        }
        "query" => query_cmd(store, spec, rest, cwd, out, err),
        "edit" => {
            writeln!(err, "zoxide edit: not supported by wadachi; use zi")?;
            Ok(2)
        }
        other => {
            writeln!(err, "zoxide {other}: not supported by wadachi")?;
            Ok(2)
        }
    }
}

fn query_cmd(
    store: &dyn DirStore,
    spec: &FrecencyRankingSpec,
    rest: &[String],
    cwd: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<i32> {
    let (mut list, mut with_score, mut all) = (false, false, false);
    let mut exclude: Vec<PathBuf> = vec![cwd.to_owned()];
    let mut keywords = Vec::new();
    let mut it = rest.iter();
    let mut literal = false;
    while let Some(a) = it.next() {
        if literal || !a.starts_with('-') {
            keywords.push(a.clone());
            continue;
        }
        match a.as_str() {
            "--" => literal = true,
            "-l" | "--list" => list = true,
            "-s" | "--score" => with_score = true,
            "-a" | "--all" => all = true,
            "-ls" | "-sl" => (list, with_score) = (true, true),
            "--exclude" => exclude.extend(it.next().map(PathBuf::from)),
            "-i" | "--interactive" => {
                writeln!(err, "zoxide query -i: use zi")?;
                return Ok(2);
            }
            other => {
                writeln!(err, "zoxide query: unsupported flag {other}")?;
                return Ok(2);
            }
        }
    }
    let ranked = {
        let mut spec = spec.clone();
        spec.phases
            .retain(|p| !matches!(p, crate::wadachi_spec::RankPhase::TopK { .. }));
        query::top_n(store, &spec, "", usize::MAX)?
    };
    let hits = ranked.into_iter().filter(|r| {
        !exclude.contains(&r.path) && (all || r.path.is_dir()) && matches_all(&r.path, &keywords)
    });
    let mut any = false;
    for r in hits {
        any = true;
        if with_score {
            write!(out, "{:>6.1} ", r.score)?;
        }
        writeln!(out, "{}", r.path.display())?;
        if !list {
            break;
        }
    }
    Ok(i32::from(!any))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemDirStore;

    fn call(store: &MemDirStore, args: &[&str], cwd: &Path) -> (i32, String) {
        let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let rc = run(
            store,
            &FrecencyRankingSpec::praca_parity(),
            &args,
            cwd,
            &mut o,
            &mut e,
        )
        .unwrap();
        (rc, String::from_utf8(o).unwrap())
    }

    #[test]
    fn add_query_remove_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("alpha").join("repo");
        std::fs::create_dir_all(&a).unwrap();
        let a = a.canonicalize().unwrap();
        let s = MemDirStore::new();
        assert_eq!(call(&s, &["add", a.to_str().unwrap()], Path::new("/")).0, 0);
        let (rc, out) = call(&s, &["query", "alpha", "repo"], Path::new("/"));
        assert_eq!((rc, out.trim()), (0, a.to_str().unwrap()));
        assert_eq!(
            call(&s, &["query", "repo"], &a).0,
            1,
            "cwd is excluded, as in zoxide"
        );
        assert_eq!(call(&s, &["query", "nope"], Path::new("/")).0, 1);
        call(&s, &["remove", a.to_str().unwrap()], Path::new("/"));
        assert_eq!(call(&s, &["query", "-l"], Path::new("/")).0, 1);
    }

    #[test]
    fn keywords_match_in_order_and_last_hits_basename() {
        let p = Path::new("/code/github/pleme-io/wadachi");
        assert!(matches_all(p, &["pleme".into(), "wad".into()]));
        assert!(!matches_all(p, &["wad".into(), "pleme".into()]));
        assert!(!matches_all(p, &["github".into()]));
    }

    #[test]
    fn unsupported_is_exit_2() {
        let s = MemDirStore::new();
        assert_eq!(call(&s, &["query", "-i"], Path::new("/")).0, 2);
        assert_eq!(call(&s, &["edit"], Path::new("/")).0, 2);
        assert_eq!(
            call(&s, &["init", "zsh", "--cmd", "cd"], Path::new("/")).0,
            0
        );
    }
}
