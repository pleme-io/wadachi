# wadachi (轍)

A native typed-Rust **directory frecency** primitive — records the directories
you walk and serves a fuzzy, frecency-ranked `cd` from a shared SQLite store.
A zoxide replacement, not a wrapper. Ranking is delegated to
[`wadachi-spec`](https://crates.io/crates/wadachi-spec) (one formula, fleet-wide).

```bash
wadachi add [PATH]      # record a visit (default cwd) — the chpwd hook
wadachi query [NEEDLE]  # ranked "score<TAB>path"
wadachi resolve NEEDLE  # single best path, exit 1 if none — the smart-cd hook
wadachi config-show     # effective config
```

Library:

```rust
use wadachi::{store::{DirStore, MemDirStore}, query};
use wadachi_spec::FrecencyRankingSpec;

let store = MemDirStore::new();
store.record("/code/github/pleme-io/wadachi").unwrap();
let hit = query::top_match(&store, &FrecencyRankingSpec::skimtab_parity(), "wadachi").unwrap();
assert_eq!(hit.unwrap().to_str().unwrap(), "/code/github/pleme-io/wadachi");
```

Store: SQLite (WAL) at `~/.local/share/wadachi/dirs.db` (override `WADACHI_DB`).
MIT.
