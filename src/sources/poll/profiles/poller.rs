use super::read::{PPD, TCCD, read_ppd, read_tccd};
use super::store::ProfileChange;
use crate::database;
use crate::sources::poll::Poller;
use std::collections::HashMap;
use std::time::Duration;

pub(super) struct ProfilePoller {
    /// The profile last stored for each daemon. Kept in memory after start-up: a change just sent
    /// may still be on its way to the database.
    last: HashMap<&'static str, String>,
}

impl ProfilePoller {
    /// Starts from the profiles already stored, so a restart does not store them again.
    pub(super) fn resume() -> Self {
        let mut last = HashMap::new();
        for daemon in [PPD, TCCD] {
            let stored: Option<String> = database::read(|c| {
                c.query_row(
                    "SELECT profile FROM profiles WHERE daemon = ?1 ORDER BY profile_id DESC LIMIT 1",
                    [daemon],
                    |r| r.get(0),
                )
                .ok()
            });
            if let Some(profile) = stored {
                last.insert(daemon, profile);
            }
        }
        ProfilePoller { last }
    }
}

impl Poller for ProfilePoller {
    fn every(&self) -> Duration {
        Duration::from_secs(10)
    }

    fn poll(&mut self) {
        for (daemon, read) in [(PPD, read_ppd as fn() -> _), (TCCD, read_tccd)] {
            // a daemon that does not answer keeps its last profile: no row for "unknown"
            let Some(now) = read() else { continue };
            if self.last.get(daemon) == Some(&now) {
                continue;
            }
            let from = self.last.insert(daemon, now.clone());
            database::write(ProfileChange {
                ts: database::now(),
                daemon,
                from,
                to: now,
            });
        }
    }
}
