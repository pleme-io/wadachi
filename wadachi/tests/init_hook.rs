//! The generated zsh/bash hook never writes to a shell's stderr — with wadachi
//! on PATH, and with it missing — and it records the visit.

use std::path::Path;
use std::process::Command;

fn which(bin: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(bin))
            .find(|c| c.is_file())
    })
}

fn hook(shell: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_wadachi"))
        .args(["init", shell])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    String::from_utf8(out.stdout).unwrap()
}

const ZSH_SCRIPT: &str = r#"eval "$WADACHI_HOOK"; cd "$T"; cd /; z target-dir; [ "$PWD" = "$T" ] || cd "$T"; z no-such-needle-xyz; true; sleep 1"#;
const BASH_SCRIPT: &str = r#"eval "$WADACHI_HOOK"; cd "$T"; __wadachi_hook; cd /; z target-dir; [ "$PWD" = "$T" ] || cd "$T"; __wadachi_hook; z no-such-needle-xyz; true; sleep 1"#;

#[allow(clippy::too_many_arguments)]
fn run(
    shell: &Path,
    args: &[&str],
    script: &str,
    path_env: &std::ffi::OsStr,
    db: &Path,
    home: &Path,
    hook: &str,
    target: &Path,
) -> Vec<u8> {
    let out = Command::new(shell)
        .args(args)
        .arg(script)
        .env_clear()
        .env("PATH", path_env)
        .env("HOME", home)
        .env("WADACHI_DB", db)
        .env("WADACHI_HOOK", hook)
        .env("T", target)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{} exited {:?}",
        shell.display(),
        out.status
    );
    out.stderr
}

fn check(shell_name: &str, args: &[&str], interactive_noise: bool) {
    let Some(shell) = which(shell_name) else {
        assert!(
            std::env::var_os("WADACHI_REQUIRE_SHELLS").is_none(),
            "{shell_name} required but not on PATH"
        );
        eprintln!("skip: {shell_name} not on PATH");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target-dir");
    std::fs::create_dir(&target).unwrap();
    let db = tmp.path().join("w.db");
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_wadachi"))
        .parent()
        .unwrap()
        .to_owned();
    let sys = "/usr/bin:/bin";
    let script = if shell_name == "bash" {
        BASH_SCRIPT
    } else {
        ZSH_SCRIPT
    };
    let hook = hook(shell_name);
    let mut with_path = std::ffi::OsString::from(bin_dir.as_os_str());
    with_path.push(":");
    with_path.push(sys);
    let with = run(
        &shell,
        args,
        script,
        &with_path,
        &db,
        tmp.path(),
        &hook,
        &target,
    );
    let without = run(
        &shell,
        args,
        script,
        std::ffi::OsStr::new(sys),
        &db,
        tmp.path(),
        &hook,
        &target,
    );
    for (label, err) in [("on PATH", with), ("missing", without)] {
        let err = String::from_utf8_lossy(&err);
        let err: String = if interactive_noise {
            err.lines()
                .filter(|l| !l.contains("job control") && !l.contains("no job control"))
                .collect()
        } else {
            err.into_owned()
        };
        assert!(
            err.trim().is_empty(),
            "{shell_name} hook wrote stderr with wadachi {label}: {err:?}"
        );
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM visits WHERE path = ?1",
            [target.to_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(n >= 1, "{shell_name} hook recorded no visit");
}

#[test]
fn zsh_hook_is_silent_and_records() {
    check("zsh", &["-f", "-c"], false);
}

#[test]
fn bash_hook_is_silent_and_records() {
    check("bash", &["--norc", "--noprofile", "-c"], true);
}
