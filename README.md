# wadachi (轍 — "the well-worn rut")

A native, typed-Rust **directory frecency** primitive for the pleme-io fleet.
It records the directories you walk and serves a fuzzy, frecency-ranked `cd`
from a SQLite store that **frost** (the shell), **skim-cd** (the picker), and
**mado** (the GPU terminal) all share. A zoxide *replacement*, not a wrapper —
the zoxide binary is never invoked at runtime (it survives only as a one-shot
`import-zoxide` migration source).

> *Wadachi* is the deep groove a cart wheel wears into a frequently-travelled
> road — exactly what a frecency tracker measures: the paths you wear smooth.

## Why one crate, fleet-wide

Every consumer ranks through **one** formula (`wadachi-spec`), so they can never
drift:

```
                    ┌───────────────────────────┐
   cd (frost) ─────▶│                           │
   OSC-7 (mado) ───▶│   ~/.local/share/         │◀──── skim-cd (ranked picker)
   indexer ────────▶│      wadachi/dirs.db      │◀──── mado overlay picker
                    │   (SQLite, WAL, shared)   │◀──── mado MCP jump tool
                    └────────────┬──────────────┘
                                 │  entries()
                                 ▼
                       wadachi_spec::apply   ← the one ranking formula
```

## Workspace

| crate | role |
|---|---|
| [`wadachi-spec`](./wadachi-spec) | **zero-I/O** frecency core — the TYPED-SPEC + INTERPRETER TRIPLET (typed Rust border + authored `specs/frecency.lisp` + interpreter with a mockable `Environment`). skim-tab adopts this too, deleting its private copy. |
| [`wadachi`](./wadachi) | the umbrella lib + `wadachi` CLI — the SQLite store, ranked queries, typed config, zoxide import. |

## Try it

```bash
wadachi add /code/github/pleme-io/wadachi   # record a visit (the chpwd hook)
wadachi add /code/github/pleme-io/mado
wadachi add /code/github/pleme-io/wadachi
wadachi query pleme                         # 2.0000  …/wadachi   1.0000  …/mado
wadachi resolve wad                         # …/wadachi   (the smart-cd resolver)
```

## Ranking

The default instance `skimtab-parity` is `score = Σ 1/(1+age_days)` —
behavior-identical to skim-tab's proven formula. Switch to `zoxide-parity`
(`frequency × 2^(-age/30d)`) via `WADACHI_RANKING=zoxide-parity` or config.

## License

MIT.
