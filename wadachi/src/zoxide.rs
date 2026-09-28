//! Import zoxide's database so history is kept when zoxide is configured off.
//!
//! Read natively (no `zoxide` binary): `db.zo` is bincode-1 fixint LE —
//! `u32 version`, `u64 count`, then per entry `u64 len + utf8 path`,
//! `f64 rank`, `u64 last_accessed` (unix seconds). Only version 3 is accepted;
//! any other version is refused by name rather than guessed at.
//!
//! zoxide scores `rank × bucket(age of last_accessed)`. wadachi's
//! `praca-parity` scores `visits × bucket(age of latest visit)` with the same
//! buckets, so an entry imported as `round(rank)` visits at `last_accessed`
//! reproduces zoxide's score up to that rounding.

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime};

use crate::store::DirStore;

/// The only zoxide on-disk version this reader understands.
pub const SUPPORTED_VERSION: u32 = 3;

/// One zoxide row, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoxideDir {
    pub path: String,
    pub rank: f64,
    pub last_accessed: u64,
}

/// Why a `db.zo` was refused.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ZoxideDbError {
    #[error("zoxide db version {found} is not supported (expected {SUPPORTED_VERSION})")]
    Version { found: u32 },
    #[error("zoxide db truncated at byte {at}")]
    Truncated { at: usize },
    #[error("zoxide db entry {index} path is not utf-8")]
    NotUtf8 { index: u64 },
    #[error("zoxide db has {trailing} trailing bytes after {count} entries")]
    Trailing { count: u64, trailing: usize },
}

struct Cursor<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ZoxideDbError> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.buf.len());
        let end = end.ok_or(ZoxideDbError::Truncated { at: self.at })?;
        let s = &self.buf[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, ZoxideDbError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, ZoxideDbError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, ZoxideDbError> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// Parse the bytes of a `db.zo`. An empty file is an empty database (zoxide
/// writes none until the first `add`).
///
/// # Errors
/// [`ZoxideDbError`] on an unsupported version or malformed bytes.
pub fn parse(buf: &[u8]) -> Result<Vec<ZoxideDir>, ZoxideDbError> {
    if buf.is_empty() {
        return Ok(Vec::new());
    }
    let mut c = Cursor { buf, at: 0 };
    let version = c.u32()?;
    if version != SUPPORTED_VERSION {
        return Err(ZoxideDbError::Version { found: version });
    }
    let count = c.u64()?;
    let mut out = Vec::new();
    for index in 0..count {
        let len = usize::try_from(c.u64()?).map_err(|_| ZoxideDbError::Truncated { at: c.at })?;
        let path = std::str::from_utf8(c.take(len)?)
            .map_err(|_| ZoxideDbError::NotUtf8 { index })?
            .to_owned();
        out.push(ZoxideDir {
            path,
            rank: c.f64()?,
            last_accessed: c.u64()?,
        });
    }
    if c.at != buf.len() {
        return Err(ZoxideDbError::Trailing {
            count,
            trailing: buf.len() - c.at,
        });
    }
    Ok(out)
}

/// Serialize rows in zoxide's format. The inverse of [`parse`]; tests and
/// fixtures use it so the round trip is asserted rather than assumed.
#[must_use]
pub fn encode(dirs: &[ZoxideDir]) -> Vec<u8> {
    let mut v = SUPPORTED_VERSION.to_le_bytes().to_vec();
    v.extend((dirs.len() as u64).to_le_bytes());
    for d in dirs {
        v.extend((d.path.len() as u64).to_le_bytes());
        v.extend(d.path.as_bytes());
        v.extend(d.rank.to_le_bytes());
        v.extend(d.last_accessed.to_le_bytes());
    }
    v
}

/// zoxide's default database location (`_ZO_DATA_DIR` wins, as in zoxide).
#[must_use]
pub fn default_db_path() -> Option<PathBuf> {
    std::env::var_os("_ZO_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("zoxide")))
        .map(|d| d.join("db.zo"))
}

/// Visits an imported row becomes: `round(rank)`, never less than one.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite, >= 1 and clamped to u32::MAX above"
)]
pub fn visits_for(rank: f64) -> u32 {
    let r = rank.round();
    if r.is_finite() && r >= 1.0 {
        r.min(f64::from(u32::MAX)) as u32
    } else {
        1
    }
}

fn at(last_accessed: u64) -> NaiveDateTime {
    DateTime::from_timestamp(i64::try_from(last_accessed).unwrap_or(i64::MAX), 0)
        .unwrap_or_default()
        .naive_utc()
}

/// What an import did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub dirs: usize,
    pub visits: u64,
}

impl std::fmt::Display for ImportSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "imported {} dirs as {} visits", self.dirs, self.visits)
    }
}

/// Record every row into `store`.
///
/// # Errors
/// Propagates storage failures.
pub fn import(store: &dyn DirStore, dirs: &[ZoxideDir]) -> anyhow::Result<ImportSummary> {
    let mut s = ImportSummary::default();
    for d in dirs {
        let n = visits_for(d.rank);
        store.record_visits_at(&d.path, at(d.last_accessed), n)?;
        s.dirs += 1;
        s.visits += u64::from(n);
    }
    Ok(s)
}

/// Read and parse a `db.zo` from disk.
///
/// # Errors
/// I/O failure or [`ZoxideDbError`].
pub fn read(path: &Path) -> anyhow::Result<Vec<ZoxideDir>> {
    Ok(parse(&std::fs::read(path)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query;
    use crate::store::MemDirStore;
    use wadachi_spec::FrecencyRankingSpec;

    fn row(p: &str, rank: f64, t: u64) -> ZoxideDir {
        ZoxideDir {
            path: p.into(),
            rank,
            last_accessed: t,
        }
    }

    #[test]
    fn round_trip() {
        let d = vec![row("/a", 3.0, 10), row("/b/é", 0.5, 20)];
        assert_eq!(parse(&encode(&d)).unwrap(), d);
    }

    #[test]
    fn empty_file_is_empty_db() {
        assert!(parse(&[]).unwrap().is_empty());
    }

    #[test]
    fn refuses_other_versions_and_bad_bytes() {
        let mut b = encode(&[row("/a", 1.0, 1)]);
        b[0] = 2;
        assert_eq!(parse(&b), Err(ZoxideDbError::Version { found: 2 }));
        let b = encode(&[row("/a", 1.0, 1)]);
        assert!(matches!(
            parse(&b[..b.len() - 1]),
            Err(ZoxideDbError::Truncated { .. })
        ));
        let mut b = encode(&[row("/a", 1.0, 1)]);
        b.push(0);
        assert!(matches!(parse(&b), Err(ZoxideDbError::Trailing { .. })));
    }

    #[test]
    fn imported_order_matches_zoxide_scoring() {
        let now = u64::try_from(chrono::Utc::now().timestamp()).unwrap();
        let dirs = vec![
            row("/code/old-heavy", 40.0, now - 30 * 86_400),
            row("/code/fresh-light", 4.0, now - 60),
            row("/code/day-mid", 6.0, now - 7_200),
        ];
        let bucket = |t: u64| match now - t {
            a if a < 3_600 => 4.0,
            a if a < 86_400 => 2.0,
            a if a < 604_800 => 0.5,
            _ => 0.25,
        };
        let mut want: Vec<_> = dirs
            .iter()
            .map(|d| (d.rank * bucket(d.last_accessed), d.path.clone()))
            .collect();
        want.sort_by(|a, b| b.0.total_cmp(&a.0));
        let store = MemDirStore::new();
        import(&store, &dirs).unwrap();
        let got = query::top_n(&store, &FrecencyRankingSpec::praca_parity(), "", 10).unwrap();
        let got: Vec<_> = got
            .iter()
            .map(|r| r.path.to_string_lossy().into_owned())
            .collect();
        let want: Vec<_> = want.into_iter().map(|w| w.1).collect();
        assert_eq!(got, want);
    }
}
