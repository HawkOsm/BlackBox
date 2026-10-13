# 0002: Records store themselves

**Status:** in use. Replaced an `enum Write`.

## Problem

With one writer thread (0001), the writer needs to know how to store whatever a source sends.

## Options

1. **An `enum Write`**, one variant per kind of record, and one `match` in the writer that runs
   each variant's SQL.
2. **A `Record` trait** with one method, `store(&self, &Connection)`, implemented by each source
   for its own records, next to their SQL. The writer only calls `store` (the Command pattern).

## What happened

Option 1 was built first, with this reasoning: "each record has one operation, and new kinds keep
coming, so an enum fits". That reasoning was backwards. The trade-off (the expression problem) is:

| | add a new kind | add a new operation |
|---|---|---|
| enum + `match` | edit every `match` | one new function |
| trait + impls | one new impl | edit every impl |

When new kinds keep coming and the operations are fixed, the trait is the one that grows easily.
With the enum, every new source meant editing `database/` (the enum and the `match`), so the
database had to know every source. The change came when the sources moved out of `database/`:
keeping them out was only possible with the trait.

## Decision

Option 2.

## Consequences

- `database/` names no source. A new source adds a record in its own `store.rs` and nothing in
  `database/` changes.
- Each record is tested on its own against an in-memory database.
- Only the writer thread can store: `store` needs a connection that may write, and sources get a
  read-only one (`database::read`).
- A queued record is a `Box<dyn Record>`, one allocation per record. Negligible at these rates, and
  it keeps every queue slot small (the enum's slot was as big as its largest variant, ~250 bytes).
