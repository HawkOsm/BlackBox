# Design decisions

One file per decision that had real alternatives: the problem, the options, what was chosen and
why, and what it costs. The short version of each is a row in the "Design decisions" table in
[architecture.md](../architecture.md); these files keep the reasoning, including the options that
lost and the mistakes made on the way, which a table has no room for.

| # | Decision | Status |
|---|---|---|
| [0001](0001-one-writer-thread.md) | One writer thread commits for every source | in use |
| [0002](0002-records-store-themselves.md) | Records store themselves (a `Record` trait, not an `enum`) | in use; replaced an `enum Write` |
| [0003](0003-sources-grouped-by-how-data-arrives.md) | Sources grouped by how their data arrives, one loop per group | in use |
| [0004](0004-severity-is-an-enum.md) | Severity is an enum, not text | in use |
| [0005](0005-a-supervisor-restarts-sources.md) | A supervisor restarts a source that panics, with backoff | in use |

A new one gets the next number. A decision that is reversed keeps its file, with its status
changed and a link to the one that replaced it: why something was tried and dropped is worth as
much as why the current way was chosen.
