# 0004: Severity is an enum

**Status:** in use.

## Problem

Every source wrote its severity as text, `"error"`, `"warning"` or `"info"`, about 25 places, plus
two SQL statements with `'info'` written into them. A misspelling (`"eror"`) compiled and stored a
row the viewer could not place.

## Options

1. **Constants** (`const ERROR: &str = "error"`): fewer typos, but any text still compiles.
2. **An enum** (`Severity::Info`, `Warning`, `Error`) that only becomes text at the database.

## Decision

Option 2, `database/severity.rs`. It implements `ToSql`, so it is passed to SQL as a parameter, and
the database still stores the same text: the viewer did not change.

## Consequences

- A wrong severity does not compile.
- The SQL statements that set one (marking shutdown noise, re-rating routine lines) take it as a
  parameter instead of a literal.
- The same thinking split auditd's parsing into "what is this record" (`Found`, an enum) and "how
  much does it matter" (`Found::alert`), which replaced a comparison against the text `"bb_modules"`
  with a `match`.
