//! The database: the connection, its schema, the writer thread and the ring-buffer trim. It knows
//! no source: each source keeps its own records and their SQL in `sources/<group>/<name>/store.rs`,
//! and hands them to `write()`.
//!
//! - `connection.rs`: opening the database, and the read-only connection behind `read()`.
//! - `writer.rs`: `Record`, the queue and the thread that commits it.
//! - `sql.rs`: what every source's SQL shares: `exists`, `index` (the alert row), `last_ts`.
//! - `time.rs`: `now`, and timestamps as the database stores them.
//! - `severity.rs`: `Severity`.
//! - `scheme.rs`: every table.
//! - `state.rs`: small values the service keeps between runs (journal cursors).
//! - `retention.rs`: the 50 GB ring buffer.

mod connection;
mod retention;
mod scheme;
mod severity;
mod sql;
mod state;
mod time;
mod writer;

use connection::conn;
#[cfg(test)]
pub(crate) use connection::open;
pub use connection::{db_path, init_database, read};
pub use retention::start_trim_thread;
pub use severity::Severity;
#[cfg(test)]
pub(crate) use sql::count;
pub use sql::{exists, index, last_ts};
pub use state::{SetState, get_state, upsert_state};
pub use time::{now, ts_epoch, ts_text};
#[cfg(test)]
pub(crate) use writer::commit;
pub use writer::{Record, drain, start_writer, write};

#[cfg(test)]
mod tests;
