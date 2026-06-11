//! SQLite-backed directory frecency store (WAL mode) — the real inter-process
//! bus. Mirrors skim-tab's `HistoryDb` shape, re-keyed to the absolute path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, NaiveDateTime};
use rusqlite::Connection;
use wadachi_spec::DirEntry;

use super::DirStore;

/// `SQLite` directory-frecency database. Real-visit events live in `visits`
/// (an append-only log); indexer-discovered dirs live in a *separate* upsert
/// table `discovered`, so an indexed dir gets only the floor score and can
/// never pollute real-visit frequency.
pub struct DirFrecencyDb {
    conn: Connection,
}

impl DirFrecencyDb {
    /// Open (creating if absent) the database at `path`, ensuring the parent
    /// directory and schema exist.
    ///
    /// # Errors
    /// Fails if the parent dir can't be created or `SQLite` can't open/migrate.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| "creating wadachi data directory")?;
        }
        let conn = Connection::open(path).with_context(|| "opening wadachi db")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Open a private in-memory database (handy for tests of the real schema).
    ///
    /// # Errors
    /// Fails if `SQLite` can't initialize.
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    fn migrate(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS visits (
                 id        INTEGER PRIMARY KEY AUTOINCREMENT,
                 path      TEXT    NOT NULL,
                 timestamp INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE TABLE IF NOT EXISTS discovered (
                 path       TEXT    PRIMARY KEY,
                 first_seen INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS idx_visits_path ON visits(path);
             CREATE INDEX IF NOT EXISTS idx_visits_ts   ON visits(timestamp);",
        )?;
        Ok(())
    }
}

impl DirStore for DirFrecencyDb {
    fn record(&self, path: &str) -> anyhow::Result<()> {
        self.conn
            .execute("INSERT INTO visits (path) VALUES (?1)", [path])?;
        Ok(())
    }

    fn record_discovered(&self, path: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO discovered (path) VALUES (?1)",
            [path],
        )?;
        Ok(())
    }

    fn entries(&self) -> anyhow::Result<Vec<DirEntry>> {
        // Group visit timestamps by path.
        let mut by_path: BTreeMap<String, Vec<NaiveDateTime>> = BTreeMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT path, timestamp FROM visits ORDER BY path")?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (path, ts) = row?;
                if let Some(dt) = DateTime::from_timestamp(ts, 0) {
                    by_path.entry(path).or_default().push(dt.naive_utc());
                }
            }
        }

        let mut entries: Vec<DirEntry> = by_path
            .into_iter()
            .map(|(path, visits)| DirEntry {
                path: PathBuf::from(path),
                visits,
                discovered_only: false,
            })
            .collect();

        // Discovered-only dirs that have no real visit.
        let visited: std::collections::HashSet<PathBuf> =
            entries.iter().map(|e| e.path.clone()).collect();
        {
            let mut stmt = self.conn.prepare("SELECT path FROM discovered")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for row in rows {
                let p = PathBuf::from(row?);
                if !visited.contains(&p) {
                    entries.push(DirEntry {
                        path: p,
                        visits: Vec::new(),
                        discovered_only: true,
                    });
                }
            }
        }

        Ok(entries)
    }

    fn discovered_under(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        // Subtree match via byte-range, not LIKE — paths containing `%`/`_`
        // would corrupt a LIKE pattern. `'0'` is the byte after `'/'`, so
        // `prefix || '/' <= path < prefix || '0'` is exactly "under prefix"
        // under SQLite's default binary collation.
        let mut stmt = self.conn.prepare(
            "SELECT path FROM discovered
             WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        )?;
        let rows = stmt.query_map([prefix], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn remove_discovered(&self, path: &str) -> anyhow::Result<()> {
        // Same byte-range subtree match as `discovered_under`.
        self.conn.execute(
            "DELETE FROM discovered
             WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
            [path],
        )?;
        Ok(())
    }
}
