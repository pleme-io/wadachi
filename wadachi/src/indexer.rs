//! `ashiato-niwa` (足跡庭 — "the footprint garden") — the M4 background
//! directory indexer. It walks the configured roots collecting *directories
//! only*, upserts them into the store's separate `discovered` table (floored
//! to `indexed_epsilon` by the ranking spec, so an indexed-but-never-visited
//! dir is rankable yet can never outrank a single real visit), prunes rows
//! whose dir no longer exists, and — in daemon mode — keeps the index live
//! via a `notify` watcher plus gate-checked periodic re-walks.
//!
//! ## Shape
//!
//! - [`IgnoreSet`] — typed never-descend set (`.git`, `node_modules`, … +
//!   hidden dirs unless `index_hidden`).
//! - [`walk_root`] — bounded iterative walk; never follows symlinks, so
//!   cycles are impossible by construction.
//! - [`walk_roots`] — bounded-parallel walk over the configured roots.
//!   **Named interim** (per the org interim-step rule): this is a typed
//!   in-process work-queue (`Mutex<VecDeque<_>>` + scoped worker threads),
//!   standing in for the destination — a `shigoto` Dag of `WalkRootJob`s.
//!   Pulling shigoto in today drags the full tokio + gen-platform closure
//!   into every shell-linked consumer (frost links this crate in-process on
//!   the `cd` hot path), so the queue stays until shigoto is consumable
//!   without that weight (or behind a feature gate). Documented in CLAUDE.md.
//! - [`RootMtimeGate`] — re-walk a root only when its mtime says its direct
//!   children changed since the last walk, or the rewalk interval (the prune
//!   staleness bound) has elapsed.
//! - [`index_once`] — one full pass: walk + upsert + prune. The `wadachi
//!   index` CLI surface.
//! - [`run_daemon`] — initial full pass, then `notify` events (debounced)
//!   for incremental upserts/removals, plus gate-checked re-walks. The
//!   `wadachi indexd` CLI surface.
//!
//! The single caller thread owns every store write; walker threads only
//! touch the filesystem. That keeps [`DirStore`]'s `&self` contract honest
//! without demanding `Sync` from the `SQLite` connection.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use notify::{RecursiveMode, Watcher};

use crate::config::{IndexerConfig, IndexerRoot};
use crate::store::DirStore;

/// Flush a pending event batch early once it reaches this size, even if the
/// debounce window never goes quiet — bounds staleness under event storms.
const MAX_PENDING_EVENTS: usize = 4096;

/// Check the re-walk gates at least this often (the loop's coarse tick);
/// the [`RootMtimeGate`] decides which roots actually get walked.
const GATE_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Typed never-descend set. A dir whose *name* matches is neither recorded
/// nor entered; hidden (`.`-prefixed) dirs are skipped unless `index_hidden`.
#[derive(Debug, Clone)]
pub struct IgnoreSet {
    names: BTreeSet<String>,
    index_hidden: bool,
}

impl IgnoreSet {
    /// Build from explicit parts.
    #[must_use]
    pub fn new(names: impl IntoIterator<Item = String>, index_hidden: bool) -> Self {
        Self {
            names: names.into_iter().collect(),
            index_hidden,
        }
    }

    /// The configured ignore surface of `cfg`.
    #[must_use]
    pub fn from_config(cfg: &IndexerConfig) -> Self {
        Self::new(cfg.ignore_names.iter().cloned(), cfg.index_hidden)
    }

    /// `true` when a dir named `name` must be skipped (and never descended).
    #[must_use]
    pub fn skips(&self, name: &str) -> bool {
        (!self.index_hidden && name.starts_with('.')) || self.names.contains(name)
    }
}

/// Walk `root` collecting directories only, up to `max_depth` levels below it
/// (1 = direct children). Ignored / hidden names are never descended into;
/// symlinks are never followed (`DirEntry::file_type` reports the link
/// itself), so cycles are unrepresentable. Unreadable dirs are skipped — the
/// walk is best-effort over whatever the filesystem yields.
#[must_use]
pub fn walk_root(root: &Path, max_depth: usize, ignore: &IgnoreSet) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth >= max_depth {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            if ignore.skips(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let path = entry.path();
            found.push(path.clone());
            stack.push((path, depth + 1));
        }
    }
    found
}

/// Walk every root with bounded parallelism, returning each root paired with
/// its live directory set. Pure filesystem reads — no store access — so the
/// caller thread keeps sole ownership of writes.
///
/// **Named interim:** typed in-process work-queue standing in for a
/// `shigoto` Dag of `WalkRootJob`s (see the module docs for why).
#[must_use]
pub fn walk_roots(
    roots: &[IndexerRoot],
    ignore: &IgnoreSet,
    concurrency: usize,
) -> Vec<(IndexerRoot, Vec<PathBuf>)> {
    let queue: Mutex<VecDeque<IndexerRoot>> = Mutex::new(roots.iter().cloned().collect());
    let done: Mutex<Vec<(IndexerRoot, Vec<PathBuf>)>> = Mutex::new(Vec::new());
    let workers = concurrency.clamp(1, roots.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().unwrap().pop_front();
                    let Some(root) = next else { break };
                    let dirs = walk_root(&root.path, root.max_depth, ignore);
                    done.lock().unwrap().push((root, dirs));
                }
            });
        }
    });
    done.into_inner().unwrap()
}

/// Per-root staleness gate: a root is re-walked only when its mtime shows
/// direct-children churn since the last walk, or the rewalk interval (the
/// prune staleness bound) has elapsed, or it has never been walked.
#[derive(Debug)]
pub struct RootMtimeGate {
    rewalk_interval: Duration,
    last_walk: HashMap<PathBuf, SystemTime>,
}

impl RootMtimeGate {
    /// A gate whose staleness bound is `rewalk_interval`.
    #[must_use]
    pub fn new(rewalk_interval: Duration) -> Self {
        Self {
            rewalk_interval,
            last_walk: HashMap::new(),
        }
    }

    /// Record that `root` was fully walked at `at`.
    pub fn mark_walked(&mut self, root: &Path, at: SystemTime) {
        self.last_walk.insert(root.to_path_buf(), at);
    }

    /// Should `root` be re-walked now?
    #[must_use]
    pub fn stale(&self, root: &Path, now: SystemTime) -> bool {
        let Some(last) = self.last_walk.get(root) else {
            return true; // never walked
        };
        if now
            .duration_since(*last)
            .map_or(true, |since| since >= self.rewalk_interval)
        {
            return true; // staleness bound hit (or clock went backwards)
        }
        match std::fs::metadata(root).and_then(|m| m.modified()) {
            Ok(mtime) => mtime > *last,
            Err(_) => true, // unreadable / vanished root — let the walk prune
        }
    }
}

/// What one indexing pass did — rendered via `Display` (the typed emission
/// surface; the CLI prints this, the library never does).
// `Copy` was dropped when `watch_degraded` was added: naming WHICH root lost
// its live watch is what makes the degradation actionable, and a count alone
// would have preserved `Copy` at the cost of the only useful detail. Verified
// against the whole workspace — no consumer relied on implicit copies.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IndexSummary {
    /// Roots actually walked this pass (gate-skipped roots don't count).
    pub roots_walked: usize,
    /// Live directories upserted into `discovered` (idempotent re-upserts
    /// included).
    pub indexed: usize,
    /// Dead `discovered` rows removed because their dir no longer exists.
    pub pruned: usize,
    /// Roots whose RECURSIVE WATCH could not be installed because some path
    /// beneath them is unreadable. Empty in the normal case.
    ///
    /// These roots are still indexed — the periodic re-walk covers them — but
    /// their updates arrive on the re-walk interval instead of immediately.
    /// Reported rather than logged because this module never prints (see the
    /// module docs); the CLI renders it.
    #[cfg_attr(feature = "serde", serde(default))]
    pub watch_degraded: Vec<PathBuf>,
}

impl fmt::Display for IndexSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "indexed {} dirs across {} roots ({} pruned)",
            self.indexed, self.roots_walked, self.pruned
        )?;
        if !self.watch_degraded.is_empty() {
            write!(
                f,
                " [live watch unavailable on {} root(s); \
                 updates arrive on the re-walk interval]",
                self.watch_degraded.len()
            )?;
        }
        Ok(())
    }
}

/// Should a failure to install a recursive watch DEGRADE the daemon rather
/// than kill it?
///
/// ── ★ ONE UNREADABLE DESCENDANT IS NOT A DEAF WATCHER ──────────────────
/// `run_daemon` used to propagate every `watch()` error, justified as "a deaf
/// watcher is worse than a dead daemon — systemd restarts us". The first half
/// is right; the second assumed the restart could FIX it.
///
/// MEASURED on rio 2026-08-05: `wadachi-daemon.service` (a systemd USER unit)
/// had restarted **232 times** and was still going, ~1374 journal lines per
/// ten minutes, every one of them:
///
/// ```text
/// Error: /home/drzzln
/// Caused by:
///     Permission denied (os error 13)
///       about ["/home/drzzln/.kube/cache/discovery/127.0.0.1_6443/cilium.io"]
/// ```
///
/// The root `/home/drzzln` is perfectly watchable. ONE descendant — a stale
/// kube discovery-cache directory left by the cilium→flannel migration — is
/// unreadable, and inotify fails the whole recursive install because of it.
/// So "a configured root can't be watched" was simply false, and the
/// permission bit is permanent: every restart re-entered the identical
/// failure. Fail-fast produced an infinite crashloop, never a recovery.
///
/// Degrading is safe HERE specifically because this daemon already re-walks
/// on `RootMtimeGate` (floor 60s, `run_daemon`). Losing watch coverage costs
/// LATENCY, not correctness. A dead daemon costs both — which is why the
/// original trade-off inverts once you notice the re-walk exists.
///
/// The split is deliberately narrow, exactly as `sentinela`'s `exec_err`
/// splits ENOENT from transient io: only a permission denial degrades. A
/// malformed path, a watch-descriptor exhaustion or a backend failure is a
/// real misconfiguration and still fails fast, because for those a restart
/// (or an operator) genuinely can change the outcome.
#[must_use]
pub fn watch_error_is_degradable(err: &notify::Error) -> bool {
    match &err.kind {
        notify::ErrorKind::Io(io) => io.kind() == std::io::ErrorKind::PermissionDenied,
        // `MaxFilesWatch` is inotify's watch-descriptor ceiling. Deliberately
        // NOT degradable: it is a real, operator-fixable resource limit
        // (`fs.inotify.max_user_watches`), and silently running blind on it
        // would hide a condition that a sysctl actually resolves.
        _ => false,
    }
}

/// What [`apply_event_path`] decided for one watcher path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOutcome {
    /// Path is a live, admissible dir — upserted into `discovered`.
    Upserted,
    /// Path is gone — removed (with its subtree) from `discovered`.
    Removed,
    /// Not ours: outside every root, too deep, ignored, or a plain file.
    Skipped,
}

/// Canonicalize every root path (best-effort — a missing root keeps its
/// configured spelling and simply walks/watches empty). This is load-bearing
/// on macOS: `FSEvents` reports *resolved* real paths (`/private/var/…`), so a
/// symlinked root spelling (`/var/…`, `$TMPDIR`, …) would never match its own
/// events. Canonicalizing at the engine border keeps walked, watched and
/// event paths in one spelling — and therefore one `discovered` row per dir.
fn canonical_roots(roots: &[IndexerRoot]) -> Vec<IndexerRoot> {
    roots
        .iter()
        .map(|r| IndexerRoot {
            path: std::fs::canonicalize(&r.path).unwrap_or_else(|_| r.path.clone()),
            max_depth: r.max_depth,
        })
        .collect()
}

/// `path` is admissible under `root` when every component below the root is
/// a normal, non-ignored name and the depth is within the root's bound.
fn admissible(root: &IndexerRoot, ignore: &IgnoreSet, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(&root.path) else {
        return false;
    };
    let mut depth = 0_usize;
    for comp in rel.components() {
        let std::path::Component::Normal(name) = comp else {
            return false;
        };
        if ignore.skips(&name.to_string_lossy()) {
            return false;
        }
        depth += 1;
    }
    (1..=root.max_depth).contains(&depth)
}

/// The deepest configured root that contains `path` (overlapping roots — e.g.
/// `$HOME` and `~/code` — resolve to the most specific one, whose depth bound
/// is the intended one).
fn deepest_root_for<'r>(roots: &'r [IndexerRoot], path: &Path) -> Option<&'r IndexerRoot> {
    roots
        .iter()
        .filter(|r| path.starts_with(&r.path))
        .max_by_key(|r| r.path.as_os_str().len())
}

/// Route one watcher path to the store: live admissible dir → upsert; gone
/// path → subtree removal; everything else → skip. This is the daemon's
/// testable event core.
///
/// # Errors
/// Propagates store failures.
pub fn apply_event_path(
    store: &impl DirStore,
    roots: &[IndexerRoot],
    ignore: &IgnoreSet,
    path: &Path,
) -> anyhow::Result<EventOutcome> {
    let Some(root) = deepest_root_for(roots, path) else {
        return Ok(EventOutcome::Skipped);
    };
    if !admissible(root, ignore, path) {
        return Ok(EventOutcome::Skipped);
    }
    let key = path.to_string_lossy();
    if path.is_dir() {
        store.record_discovered(&key)?;
        Ok(EventOutcome::Upserted)
    } else if path.exists() {
        Ok(EventOutcome::Skipped) // a file changed — not a jump target
    } else {
        store.remove_discovered(&key)?;
        Ok(EventOutcome::Removed)
    }
}

/// Upsert `live` dirs under `root` and prune `discovered` rows whose dir no
/// longer exists. Prune is scoped to this root, so staleness stays bounded
/// per root; rows discovered deeper than the walk depth (e.g. by a watcher
/// event) survive as long as their dir does.
fn index_root(
    store: &impl DirStore,
    root: &IndexerRoot,
    live: &[PathBuf],
) -> anyhow::Result<usize> {
    for dir in live {
        store.record_discovered(&dir.to_string_lossy())?;
    }
    let mut pruned = 0_usize;
    for known in store.discovered_under(&root.path.to_string_lossy())? {
        if !Path::new(&known).is_dir() {
            store.remove_discovered(&known)?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// One full indexing pass over `cfg.roots`: bounded-parallel walk, then
/// upsert + prune per root. The `wadachi index` one-shot.
///
/// # Errors
/// Propagates store failures (filesystem hiccups inside the walk are
/// best-effort skips, never errors).
pub fn index_once(store: &impl DirStore, cfg: &IndexerConfig) -> anyhow::Result<IndexSummary> {
    let ignore = IgnoreSet::from_config(cfg);
    let roots = canonical_roots(&cfg.roots);
    index_walks(store, &walk_roots(&roots, &ignore, cfg.concurrency))
}

/// Upsert + prune a finished walk set (shared by [`index_once`] and the
/// daemon's gate-checked re-walks).
fn index_walks(
    store: &impl DirStore,
    walks: &[(IndexerRoot, Vec<PathBuf>)],
) -> anyhow::Result<IndexSummary> {
    let mut summary = IndexSummary::default();
    for (root, live) in walks {
        summary.roots_walked += 1;
        summary.indexed += live.len();
        summary.pruned += index_root(store, root, live)?;
    }
    Ok(summary)
}

/// Re-walk (and prune under) every root the gate calls stale, marking each
/// freshly walked.
fn rewalk_stale(
    store: &impl DirStore,
    roots: &[IndexerRoot],
    concurrency: usize,
    ignore: &IgnoreSet,
    gate: &mut RootMtimeGate,
) -> anyhow::Result<IndexSummary> {
    let now = SystemTime::now();
    let stale: Vec<IndexerRoot> = roots
        .iter()
        .filter(|r| gate.stale(&r.path, now))
        .cloned()
        .collect();
    if stale.is_empty() {
        return Ok(IndexSummary::default());
    }
    let summary = index_walks(store, &walk_roots(&stale, ignore, concurrency))?;
    for root in &stale {
        gate.mark_walked(&root.path, now);
    }
    Ok(summary)
}

/// The `wadachi indexd` daemon loop: one initial full pass, then a `notify`
/// watcher on every root (recursive) feeding debounced incremental
/// upserts/removals, plus gate-checked periodic re-walks (the prune
/// staleness bound). `on_pass` observes every pass that walked at least one
/// root — the CLI renders it via [`IndexSummary`]'s `Display`; the library
/// itself never prints.
///
/// Runs until the watcher channel disconnects (process teardown).
///
/// # Errors
/// Fails fast if the watcher can't be created or a configured root can't be
/// watched (a deaf watcher is worse than a dead daemon — launchd/systemd
/// restarts us), and propagates store failures.
pub fn run_daemon(
    store: &impl DirStore,
    cfg: &IndexerConfig,
    mut on_pass: impl FnMut(&IndexSummary),
) -> anyhow::Result<()> {
    let ignore = IgnoreSet::from_config(cfg);
    // One canonical root set for walking, watching, event matching and the
    // gate — so every surface speaks the same path spelling (see
    // `canonical_roots` for why this is load-bearing on macOS).
    let roots = canonical_roots(&cfg.roots);
    let mut gate = RootMtimeGate::new(Duration::from_secs(cfg.rewalk_interval_secs.max(60)));

    let mut initial = index_walks(store, &walk_roots(&roots, &ignore, cfg.concurrency))?;
    let walked_at = SystemTime::now();
    for root in &roots {
        gate.mark_walked(&root.path, walked_at);
    }

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event); // receiver gone == daemon tearing down
    })
    .context("creating filesystem watcher")?;
    // A permission denial somewhere beneath a root degrades that root to
    // re-walk-only; anything else still fails fast. See
    // `watch_error_is_degradable` for the rio measurement that motivated the
    // split — the previous unconditional `?` here crashlooped 232 times on a
    // single unreadable stale cache directory.
    for root in &roots {
        if root.path.is_dir() {
            match watcher.watch(&root.path, RecursiveMode::Recursive) {
                Ok(()) => {}
                Err(e) if watch_error_is_degradable(&e) => {
                    initial.watch_degraded.push(root.path.clone());
                }
                Err(e) => {
                    return Err(anyhow::Error::new(e).context(root.path.display().to_string()));
                }
            }
        }
    }
    // Reported after the watch install so the first pass can carry the
    // coverage verdict; the walk itself already happened above.
    on_pass(&initial);

    let debounce = Duration::from_millis(cfg.debounce_ms.clamp(50, 10_000));
    let mut pending: BTreeSet<PathBuf> = BTreeSet::new();
    let mut last_gate_check = Instant::now();
    loop {
        match rx.recv_timeout(debounce) {
            Ok(Ok(event)) => {
                pending.extend(event.paths);
                if pending.len() < MAX_PENDING_EVENTS {
                    continue; // keep absorbing until the window goes quiet
                }
            }
            Ok(Err(_)) => continue, // backend hiccup — the re-walk self-heals
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
        for path in std::mem::take(&mut pending) {
            apply_event_path(store, &roots, &ignore, &path)?;
        }
        if last_gate_check.elapsed() >= GATE_CHECK_INTERVAL {
            let pass = rewalk_stale(store, &roots, cfg.concurrency, &ignore, &mut gate)?;
            if pass.roots_walked > 0 {
                on_pass(&pass);
            }
            last_gate_check = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignore() -> IgnoreSet {
        IgnoreSet::new(["node_modules".to_owned(), "target".to_owned()], false)
    }

    #[test]
    fn ignore_set_skips_named_and_hidden() {
        let ig = ignore();
        assert!(ig.skips("node_modules"));
        assert!(ig.skips(".git")); // hidden, even though not listed
        assert!(!ig.skips("src"));
        let permissive = IgnoreSet::new(["target".to_owned()], true);
        assert!(!permissive.skips(".config")); // index_hidden lets dotdirs in
        assert!(permissive.skips("target"));
    }

    #[test]
    fn admissible_enforces_depth_ignore_and_root() {
        let root = IndexerRoot::new("/r", 2);
        let ig = ignore();
        assert!(admissible(&root, &ig, Path::new("/r/a")));
        assert!(admissible(&root, &ig, Path::new("/r/a/b")));
        assert!(!admissible(&root, &ig, Path::new("/r"))); // the root itself
        assert!(!admissible(&root, &ig, Path::new("/r/a/b/c"))); // too deep
        assert!(!admissible(&root, &ig, Path::new("/r/target/x"))); // ignored component
        assert!(!admissible(&root, &ig, Path::new("/elsewhere/a"))); // outside
    }

    #[test]
    fn deepest_root_wins_for_overlapping_roots() {
        let roots = vec![
            IndexerRoot::new("/home/u", 2),
            IndexerRoot::new("/home/u/code", 6),
        ];
        let hit = deepest_root_for(&roots, Path::new("/home/u/code/gh/org/repo")).unwrap();
        assert_eq!(hit.path, PathBuf::from("/home/u/code"));
    }

    #[test]
    fn mtime_gate_interval_and_unwalked() {
        let mut gate = RootMtimeGate::new(Duration::from_secs(900));
        let root = Path::new("/nonexistent-gate-root");
        let now = SystemTime::now();
        assert!(gate.stale(root, now)); // never walked
        gate.mark_walked(root, now);
        // Within the interval the (unreadable) root falls back to stale=true;
        // a readable, unchanged root is exercised in the integration tests.
        assert!(gate.stale(root, now + Duration::from_secs(1)));
        assert!(gate.stale(root, now + Duration::from_secs(901))); // bound hit
    }

    // ── watch_error_is_degradable: the rio 232-restart crashloop ─────────
    //
    // Regression tests for wadachi-daemon.service restarting 232 times on a
    // single unreadable stale kube-cache directory beneath /home/drzzln.

    #[test]
    fn permission_denied_degrades_instead_of_killing_the_daemon() {
        let e = notify::Error::io(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(
            watch_error_is_degradable(&e),
            "one unreadable descendant must NOT kill a daemon that already \
             re-walks on an interval — the permission bit is permanent, so \
             every restart re-enters the identical failure"
        );
    }

    #[test]
    fn other_io_errors_still_fail_fast() {
        // A restart (or an operator) can plausibly change these, so the
        // original fail-fast is still correct for them.
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::InvalidInput,
        ] {
            let e = notify::Error::io(std::io::Error::from(kind));
            assert!(
                !watch_error_is_degradable(&e),
                "{kind:?} must keep failing fast — degrading it would run \
                 blind on a condition someone can actually fix"
            );
        }
    }

    #[test]
    fn watch_descriptor_exhaustion_is_not_degradable() {
        // inotify's fs.inotify.max_user_watches ceiling is operator-fixable;
        // silently running blind on it would hide a sysctl-shaped problem.
        let e = notify::Error::new(notify::ErrorKind::MaxFilesWatch);
        assert!(!watch_error_is_degradable(&e));
    }

    #[test]
    fn summary_display_names_the_degradation_and_stays_quiet_otherwise() {
        // The module never prints, so this Display IS the operator channel.
        let clean = IndexSummary {
            roots_walked: 2,
            indexed: 9,
            pruned: 1,
            watch_degraded: Vec::new(),
        };
        assert!(
            !clean.to_string().contains("watch"),
            "a healthy pass must not mention watches: {clean}"
        );

        let degraded = IndexSummary {
            roots_walked: 2,
            indexed: 9,
            pruned: 1,
            watch_degraded: vec![PathBuf::from("/home/drzzln")],
        };
        let rendered = degraded.to_string();
        assert!(
            rendered.contains("live watch unavailable"),
            "degradation must be visible, not silent: {rendered}"
        );
        assert!(
            rendered.contains("re-walk"),
            "and must say why it is still correct: {rendered}"
        );
    }
}
