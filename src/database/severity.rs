use rusqlite::types::{ToSql, ToSqlOutput};

/// How much an event matters, as stored in `messages.severity` and shown by the viewer. An enum,
/// not text: a misspelt severity is a compile error instead of a row the viewer cannot place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    /// The text the database and the viewer use.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

impl ToSql for Severity {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}
