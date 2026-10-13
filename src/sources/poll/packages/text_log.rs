use super::{History, Parser, Transaction};
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

/// A log that never marks the end of a transaction (zypper) has its open one stored once the log
/// has been quiet this long. Long enough to outlast a kernel's initramfs rebuild between two lines.
const QUIET: Duration = Duration::from_secs(300);

/// A text log (pacman, apt, zypper), read from the top once, then from where the last read ended.
pub(super) struct TextLog<P> {
    name: &'static str,
    path: String,
    since: i64,
    parser: P,
    offset: u64,
    /// A line still being written, kept until its newline arrives.
    pending: String,
    grew: Instant,
}

impl<P: Parser + Send + 'static> TextLog<P> {
    pub(super) fn open(name: &'static str, path: String, since: i64) -> Box<dyn History> {
        Box::new(TextLog {
            name,
            path,
            since,
            parser: P::default(),
            offset: 0,
            pending: String::new(),
            grew: Instant::now(),
        })
    }
}

impl<P: Parser> TextLog<P> {
    /// The transactions completed by the lines appended since the last read. A half-written last
    /// line waits for the next read.
    fn read_new(&mut self, mut file: std::fs::File) -> Result<Vec<Transaction>, String> {
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            // rotated or truncated: start over from the top
            self.offset = 0;
            self.pending.clear();
            self.parser = P::default();
        }
        if len == self.offset {
            return Ok(Vec::new());
        }
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(self.offset))
            .and_then(|_| file.read_to_end(&mut bytes))
            .map_err(|e| e.to_string())?;
        self.offset += bytes.len() as u64;
        self.grew = Instant::now();
        self.pending.push_str(&String::from_utf8_lossy(&bytes));
        let complete = self.pending.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let found = self.pending[..complete]
            .lines()
            .filter_map(|line| self.parser.line(line))
            .collect();
        self.pending.drain(..complete);
        Ok(found)
    }
}

impl<P: Parser + Send> History for TextLog<P> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn poll(&mut self) -> Result<Vec<Transaction>, String> {
        let mut found = match std::fs::File::open(&self.path) {
            Ok(file) => self.read_new(file)?,
            // gone for a moment while it is rotated: nothing new yet
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.to_string()),
        };
        // a log that never marks the end of a transaction (zypper): close the open one once the
        // log has been quiet long enough
        if self.grew.elapsed() >= QUIET {
            found.extend(self.parser.quiet());
        }
        found.retain(|t| t.ts >= self.since);
        Ok(found)
    }

    fn history(&self) -> Result<Vec<Transaction>, String> {
        let bytes = std::fs::read(&self.path).map_err(|e| e.to_string())?;
        Ok(read_all::<P>(&String::from_utf8_lossy(&bytes)))
    }
}

/// Every transaction in a whole text log, the open one at the end included.
pub(super) fn read_all<P: Parser>(text: &str) -> Vec<Transaction> {
    let mut parser = P::default();
    let mut found: Vec<Transaction> = text.lines().filter_map(|l| parser.line(l)).collect();
    found.extend(parser.quiet());
    found
}

#[cfg(test)]
mod tests {
    use super::super::pacman;
    use super::*;

    #[test]
    fn a_text_log_is_read_as_it_grows() {
        use std::io::Write as _;
        let run = |time: &str, package: &str| {
            format!(
                "[2026-09-22T{time}+0300] [ALPM] transaction started\n\
                 [2026-09-22T{time}+0300] [ALPM] installed {package} (1-1)\n\
                 [2026-09-22T{time}+0300] [ALPM] transaction completed\n"
            )
        };
        let changes = |found: Vec<Transaction>| -> Vec<String> {
            found.into_iter().flat_map(|t| t.changes).collect()
        };
        let path = std::env::temp_dir().join(format!("bb_textlog_{}.log", std::process::id()));
        std::fs::write(
            &path,
            run("22:00:00", "old") + &run("22:10:00", "same-second"),
        )
        .unwrap();
        // stored up to 22:10:00 +03:00: the run before it is skipped, the one in that second is
        // read again (the writer drops it if it is the same)
        let mut log = TextLog::<pacman::Parser>::open(
            "pacman",
            path.to_string_lossy().into_owned(),
            1_790_104_200,
        );
        assert_eq!(
            changes(log.poll().unwrap()),
            ["installed same-second (1-1)"]
        );
        assert!(log.poll().unwrap().is_empty(), "nothing new");

        let append = |text: &str| {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            f.write_all(text.as_bytes()).unwrap();
        };
        let new = run("22:20:00", "new");
        let (head, tail) = new.split_at(new.len() - 6);
        append(head);
        assert!(log.poll().unwrap().is_empty(), "a half-written line waits");
        append(tail);
        assert_eq!(changes(log.poll().unwrap()), ["installed new (1-1)"]);

        // rotated: the file is shorter than what was read, so it is read again from the top
        std::fs::write(&path, run("22:30:00", "after")).unwrap();
        assert_eq!(changes(log.poll().unwrap()), ["installed after (1-1)"]);

        // gone for a moment during rotation: nothing new, not an error
        std::fs::remove_file(&path).unwrap();
        assert_eq!(log.poll(), Ok(Vec::new()));
    }
}
