//! In-memory directory store — the test seam. Same observable behavior as
//! [`super::DirFrecencyDb`] without touching disk or `SQLite`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{NaiveDateTime, Utc};
use wadachi_spec::DirEntry;

use super::DirStore;

/// In-memory [`DirStore`]. Visits accumulate per path; discovered paths are a
/// separate set. Interior-mutable so it matches the `&self` trait shape.
#[derive(Default)]
pub struct MemDirStore {
    visits: Mutex<BTreeMap<String, Vec<NaiveDateTime>>>,
    discovered: Mutex<std::collections::BTreeSet<String>>,
}

impl MemDirStore {
    /// A fresh, empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a visit at an explicit time (tests use this for determinism).
    pub fn record_at(&self, path: &str, at: NaiveDateTime) {
        self.visits
            .lock()
            .unwrap()
            .entry(path.to_owned())
            .or_default()
            .push(at);
    }
}

impl DirStore for MemDirStore {
    fn record(&self, path: &str) -> anyhow::Result<()> {
        self.record_at(path, Utc::now().naive_utc());
        Ok(())
    }

    fn record_discovered(&self, path: &str) -> anyhow::Result<()> {
        self.discovered.lock().unwrap().insert(path.to_owned());
        Ok(())
    }

    fn entries(&self) -> anyhow::Result<Vec<DirEntry>> {
        let visits = self.visits.lock().unwrap();
        let mut entries: Vec<DirEntry> = visits
            .iter()
            .map(|(path, ts)| DirEntry {
                path: PathBuf::from(path),
                visits: ts.clone(),
                discovered_only: false,
            })
            .collect();
        let visited: std::collections::HashSet<&String> = visits.keys().collect();
        for p in self.discovered.lock().unwrap().iter() {
            if !visited.contains(p) {
                entries.push(DirEntry {
                    path: PathBuf::from(p),
                    visits: Vec::new(),
                    discovered_only: true,
                });
            }
        }
        Ok(entries)
    }

    fn discovered_under(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        Ok(self
            .discovered
            .lock()
            .unwrap()
            .iter()
            .filter(|p| super::path_is_same_or_under(p, prefix))
            .cloned()
            .collect())
    }

    fn remove_discovered(&self, path: &str) -> anyhow::Result<()> {
        self.discovered
            .lock()
            .unwrap()
            .retain(|p| !super::path_is_same_or_under(p, path));
        Ok(())
    }
}
