use super::parse::line_ts;
use std::io::{Read, Seek, SeekFrom};

/// The first line that starts at or after byte `pos`, with its start and timestamp.
fn line_at(file: &mut std::fs::File, pos: u64) -> Option<(u64, i64)> {
    let mut start = pos;
    let mut buf = vec![0u8; 16 * 1024];
    if pos > 0 {
        // the line containing pos - 1 is cut: skip past its newline
        file.seek(SeekFrom::Start(pos - 1)).ok()?;
        let n = file.read(&mut buf).ok()?;
        start = pos - 1 + buf[..n].iter().position(|&b| b == b'\n')? as u64 + 1;
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let n = file.read(&mut buf).ok()?;
    let end = buf[..n].iter().position(|&b| b == b'\n').unwrap_or(n);
    let ts = line_ts(&String::from_utf8_lossy(&buf[..end]))?;
    Some((start, ts))
}

/// Byte offset of the first record newer than `after`, found by binary search (the log is in
/// time order), so a restart picks up what happened while the collector was down without
/// reading the whole file. Records in rotated-away files are not recovered.
pub fn resume_offset(path: &str, after: i64) -> u64 {
    let Ok(mut file) = std::fs::File::open(path) else {
        return 0;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let (mut lo, mut hi) = (0u64, len);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match line_at(&mut file, mid) {
            Some((_, ts)) if ts <= after => lo = mid + 1,
            _ => hi = mid,
        }
    }
    line_at(&mut file, lo).map_or(len, |(start, _)| start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumes_after_the_last_stored_record() {
        let path = std::env::temp_dir().join(format!("bb_audit_{}.log", std::process::id()));
        let lines: Vec<String> = (0..500)
            .map(|i| {
                format!(
                    "type=SYSCALL msg=audit({}.000:{i}): pid=1 {}\n",
                    1000 + i / 2,
                    "x".repeat(i % 97)
                )
            })
            .collect();
        std::fs::write(&path, lines.concat()).unwrap();
        let p = path.to_str().unwrap();
        let offset_of = |i: usize| lines[..i].iter().map(|l| l.len() as u64).sum::<u64>();
        assert_eq!(
            resume_offset(p, 999),
            0,
            "nothing stored yet in this range: from the top"
        );
        assert_eq!(
            resume_offset(p, 1000),
            offset_of(2),
            "past both lines of second 1000"
        );
        assert_eq!(resume_offset(p, 1123), offset_of(248));
        assert_eq!(
            resume_offset(p, 5000),
            offset_of(500),
            "all stored: from the end"
        );
        let _ = std::fs::remove_file(&path);
    }
}
