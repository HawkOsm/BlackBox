use crate::database::{Record, Severity, exists, index, read, ts_text};
use rusqlite::{Connection, params};

/// A boot started, or the previous one ended without a clean shutdown.
pub struct Boot {
    pub ts: i64,
    /// `start` or `unclean_end`.
    pub kind: &'static str,
    pub kernel_boot: String,
    pub kernel: Option<String>,
    pub note: Option<String>,
    pub severity: Severity,
    pub summary: String,
}

/// Whether this boot event is already stored. If that cannot be read, it counts as not stored: a
/// boot recorded twice is better than one not recorded at all.
pub fn boot_recorded(kernel_boot: &str, kind: &str) -> bool {
    read(|c| {
        exists(
            c,
            "SELECT 1 FROM boot WHERE kernel_boot = ?1 AND kind = ?2",
            params![kernel_boot, kind],
        )
    })
    .unwrap_or_else(|e| {
        eprintln!("boot: cannot check the stored boots ({e}); recording anyway");
        false
    })
}

impl Record for Boot {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        let ts = ts_text(self.ts);
        c.execute(
            "INSERT INTO boot (ts, kind, kernel_boot, kernel, note) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, self.kind, self.kernel_boot, self.kernel, self.note],
        )?;
        index(c, "boot", &ts, Some((self.severity, &self.summary)))
    }
}

/// Errors and warnings just before a clean shutdown are programs being killed as the session is
/// torn down (Hyprland and Spotify both aborted during one poweroff here), not failures: keep them,
/// as info. Running it again changes nothing.
pub struct MarkShutdown {
    pub from: i64,
    pub to: i64,
}

impl Record for MarkShutdown {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        c.execute(
            "UPDATE messages SET severity = ?3, summary = summary || ' (during shutdown)'
             WHERE ts BETWEEN ?1 AND ?2 AND severity != ?3",
            params![ts_text(self.from), ts_text(self.to), Severity::Info],
        )
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{count, open};

    #[test]
    fn errors_in_a_clean_shutdowns_last_seconds_become_info_once() {
        let c = open(":memory:");
        c.execute_batch(
            "INSERT INTO messages (source, ref_id, ts, severity, summary) VALUES
                 ('custom', 1, '2023-11-15 12:13:20', 'error', 'start-hyprland (pid 7) killed by SIGABRT (6)'),
                 ('custom', 2, '2023-11-15 11:00:00', 'error', 'long before');",
        )
        .unwrap();
        let shutdown = MarkShutdown {
            from: 1_700_050_390,
            to: 1_700_050_410,
        };
        shutdown.store(&c).unwrap();
        shutdown.store(&c).unwrap();
        let (sev, text): (String, String) = c
            .query_row(
                "SELECT severity, summary FROM messages WHERE ref_id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sev, "info");
        assert_eq!(
            text, "start-hyprland (pid 7) killed by SIGABRT (6) (during shutdown)",
            "marked once, however often it runs"
        );
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM messages WHERE severity = 'error'"),
            1,
            "an error outside the window keeps its severity"
        );
    }
}
