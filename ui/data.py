"""Read-only queries over the blackbox database."""
import calendar
import os
import sqlite3
import time
from pathlib import Path

FMT = "%Y-%m-%d %H:%M:%S"
TABLES = {"custom": "custom_id", "journald": "journald_id", "auditd": "audit_id", "sysstat": "sysstat_id"}
SPAN = "ts BETWEEN datetime(?, ?) AND datetime(?, ?)"


def default_db():
    return os.environ.get("BLACKBOX_DB") or str(Path.home() / ".local/share/blackbox/blackbox.db")


def epoch(ts):
    return calendar.timegm(time.strptime(ts, FMT))


def collapse(rows):
    """Folds runs of the same message into one row with a count (rows arrive newest first)."""
    out = []
    for r in rows:
        last = out[-1] if out else None
        if last and (last["source"], last["severity"], last["summary"]) == (r["source"], r["severity"], r["summary"]):
            last["count"] += 1
            last["first_ts"] = r["ts"]
        else:
            out.append({**r, "count": 1, "first_ts": r["ts"]})
    return out


class Data:
    def __init__(self, path):
        self.path = path

    def rows(self, sql, args=()):
        con = sqlite3.connect(f"file:{self.path}?mode=ro", uri=True, timeout=5)
        con.row_factory = sqlite3.Row
        try:
            return [dict(r) for r in con.execute(sql, args)]
        finally:
            con.close()

    def messages(self, severities, sources, text="", since=None, limit=300):
        where, args = [], []
        for column, values in (("severity", severities), ("source", sources)):
            if not values:
                return []
            where.append(f"{column} IN ({','.join('?' * len(values))})")
            args += sorted(values)
        if text:
            escaped = text.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")
            where.append("summary LIKE ? ESCAPE '\\'")
            args.append(f"%{escaped}%")
        if since:
            where.append("ts >= ?")
            args.append(since)
        sql = (
            "SELECT source, ref_id, ts, severity, summary FROM messages WHERE "
            + " AND ".join(where)
            + " ORDER BY ts DESC, rowid DESC LIMIT ?"
        )
        # fetch extra so that collapsing a burst of repeats still leaves a full page
        return collapse(self.rows(sql, (*args, limit * 4)))[:limit]

    def context(self, ts, window=120):
        bounds = (ts, f"-{window} seconds", ts, f"+{window} seconds")
        notable = self.rows(
            f"SELECT source, ref_id, ts, severity, summary FROM messages WHERE severity != 'info' AND {SPAN} ORDER BY ts LIMIT 300",
            bounds,
        )
        nearby_info = self.rows(
            f"SELECT source, ref_id, ts, severity, summary FROM messages WHERE severity = 'info' AND {SPAN} "
            "ORDER BY abs(strftime('%s', ts) - strftime('%s', ?)) LIMIT 30",
            (*bounds, ts),
        )
        return {
            "window": window,
            "messages": sorted(notable + nearby_info, key=lambda m: m["ts"]),
            "sysstat": self.rows(
                "SELECT ts, cpu_pct, mem_pct, disk_io_kb, load_avg, gpu_pct, gpu_mem_pct, gpu_temp "
                f"FROM sysstat WHERE {SPAN} ORDER BY ts",
                bounds,
            ),
        }

    def detail(self, source, ref_id):
        if source not in TABLES:
            return {}
        found = self.rows(f"SELECT * FROM {source} WHERE {TABLES[source]} = ?", (ref_id,))
        return found[0] if found else {}

    def coredump(self, pid, ts):
        """systemd-coredump's journal entry for a crash, which carries the stack trace."""
        found = self.rows(
            "SELECT message FROM journald WHERE message LIKE ? "
            "AND ts BETWEEN datetime(?, '-60 seconds') AND datetime(?, '+600 seconds') ORDER BY ts LIMIT 1",
            (f"Process {int(pid)} (%", ts, ts),
        )
        return found[0]["message"] if found else None

    def overview(self, hours=24):
        """Load averaged into about 150 buckets, plus every notable event, for the last `hours`
        (or since the first sample, when the recording is younger than that)."""
        since = f"-{hours} hours"
        first = self.rows("SELECT MIN(ts) AS ts FROM sysstat WHERE ts >= datetime('now', ?)", (since,))[0]["ts"]
        span = time.time() - epoch(first) if first else hours * 3600
        bucket_s = max(10, int(span / 150) // 10 * 10)
        load = self.rows(
            "SELECT MIN(ts) AS ts, AVG(cpu_pct) AS cpu_pct, AVG(mem_pct) AS mem_pct, AVG(gpu_pct) AS gpu_pct "
            f"FROM sysstat WHERE ts >= datetime('now', ?) GROUP BY strftime('%s', ts) / {bucket_s} ORDER BY ts",
            (since,),
        )
        events = self.rows(
            "SELECT ts, severity, summary FROM messages WHERE severity != 'info' AND ts >= datetime('now', ?) "
            "ORDER BY ts DESC LIMIT 2000",
            (since,),
        )
        return {"hours": hours, "bucket_s": bucket_s, "load": load, "events": events}

    def last_gpu(self):
        """The newest GPU reading: the GPU is sampled every third tick, so the newest row often has none."""
        found = self.rows(
            "SELECT gpu_pct, gpu_temp FROM sysstat WHERE gpu_pct IS NOT NULL AND ts >= datetime('now', '-5 minutes') "
            "ORDER BY sysstat_id DESC LIMIT 1"
        )
        return found[0] if found else None

    def summary(self):
        counts = self.rows(
            "SELECT severity, COUNT(*) AS n FROM messages WHERE ts >= datetime('now', '-24 hours') GROUP BY severity"
        )
        by_severity = {c["severity"]: c["n"] for c in counts}
        last = self.rows("SELECT * FROM sysstat ORDER BY sysstat_id DESC LIMIT 1")
        age = None
        if last:
            age = int(time.time() - epoch(last[0]["ts"]))
        base = Path(self.path)
        size = sum(f.stat().st_size for f in base.parent.glob(base.name + "*"))
        return {
            "errors_24h": by_severity.get("error", 0),
            "warnings_24h": by_severity.get("warning", 0),
            "last_sample": last[0] if last else None,
            "sample_age_s": age,
            "db_bytes": size,
        }
