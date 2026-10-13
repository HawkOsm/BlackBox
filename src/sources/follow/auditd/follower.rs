use super::log_path;
use super::parse::parse_line;
use super::resume::resume_offset;
use crate::database;
use crate::sources::follow::Follower;
use std::process::Command;

pub(super) struct Auditd {
    pub(super) booted: Option<i64>,
    /// the newest stored record: read once, then kept here, since a record just sent may still
    /// be on its way to the database
    pub(super) last: Option<i64>,
}

impl Follower for Auditd {
    fn command(&mut self) -> Option<Command> {
        let path = log_path();
        // auditd not installed or running, or the log isn't readable: look again later
        std::fs::File::open(&path).ok()?;
        // resume after the newest stored record; the first time, start at the end
        let from = match self.last {
            Some(after) => resume_offset(&path, after) + 1,
            None => std::fs::metadata(&path).map_or(0, |m| m.len()) + 1,
        };
        let mut command = Command::new("tail");
        command.args(["-c", &format!("+{from}"), "-F", &path]);
        Some(command)
    }

    fn line(&mut self, line: &str) {
        let Some(e) = parse_line(line, self.booted) else {
            return;
        };
        self.last = Some(e.ts);
        database::write(e);
    }
}
