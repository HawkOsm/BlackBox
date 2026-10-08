"""Read-only queries over the blackbox database."""
import calendar
import os
import re
import sqlite3
import time
from pathlib import Path

FMT = "%Y-%m-%d %H:%M:%S"
TABLES = {
    "custom": "custom_id",
    "journald": "journald_id",
    "auditd": "audit_id",
    "sysstat": "sysstat_id",
    "boot": "boot_id",
    "packages": "package_id",
}
QUERY_SECONDS = 8
# chart series fed from the `sensors` table: key -> (kind, SQL LIKE pattern of the sensor name).
# Where several sensors match (two NVMe drives), the highest reading is the one charted.
SENSOR_SERIES = {
    "cpu_temp": ("temp", "coretemp/Package id%"),
    "nvme_temp": ("temp", "nvme%/Composite"),
    "ram_temp": ("temp", "spd5118%"),
    "platform_watt": ("power", "rapl/psys"),
}
SPAN = "ts BETWEEN datetime(?, ?) AND datetime(?, ?)"


def default_db():
    return os.environ.get("BLACKBOX_DB") or str(Path.home() / ".local/share/blackbox/blackbox.db")


def epoch(ts):
    return calendar.timegm(time.strptime(ts, FMT))


PID = re.compile(r"\bpid \d+")


def collapse(rows, window=600):
    """Folds repeats of the same message into one row with a count (rows arrive newest first).
    Messages that differ only in a pid count as the same, and a repeat joins its group even with
    other events in between, as long as it is within `window` seconds of the group's last one:
    a login that runs the same failing helper 90 times shows up as one row, not 90."""
    out, open_groups = [], {}
    for r in rows:
        key = (r["source"], r["severity"], PID.sub("pid", r["summary"]))
        group = open_groups.get(key)
        if group and epoch(group["first_ts"]) - epoch(r["ts"]) <= window:
            group["count"] += 1
            group["first_ts"] = r["ts"]
        else:
            group = {**r, "count": 1, "first_ts": r["ts"]}
            open_groups[key] = group
            out.append(group)
    return out


class Data:
    def __init__(self, path):
        self.path = path

    def rows(self, sql, args=()):
        con = sqlite3.connect(f"file:{self.path}?mode=ro", uri=True, timeout=5)
        con.row_factory = sqlite3.Row
        # a scan over a very large table must not run for minutes: give up and report an error
        deadline = time.monotonic() + QUERY_SECONDS
        con.set_progress_handler(lambda: time.monotonic() > deadline, 100_000)
        try:
            return [dict(r) for r in con.execute(sql, args)]
        finally:
            con.close()

    def optional_rows(self, sql, args=()):
        """Rows of a table added after the first release (`power`, `sensors`). A database written by
        an older collector does not have it yet, which means "no data", not an error."""
        try:
            return self.rows(sql, args)
        except sqlite3.OperationalError as e:
            if "no such table" in str(e):
                return []
            raise

    @staticmethod
    def _series_case():
        """A CASE expression naming the chart series a sensors row belongs to, and its arguments."""
        whens = " ".join("WHEN kind = ? AND name LIKE ? THEN ?" for _ in SENSOR_SERIES)
        args = [x for key, (kind, like) in SENSOR_SERIES.items() for x in (kind, like, key)]
        return f"CASE {whens} END", args

    def sensor_series(self, where, args, bucket_s=None):
        """Per-tick values of the charted sensors as [(ts, series key, value)], newest tick last.
        With `bucket_s` they are averaged into buckets of that many seconds."""
        case, case_args = self._series_case()
        per_tick = (
            f"SELECT ts, series, MAX(value) AS value FROM (SELECT ts, {case} AS series, value FROM sensors WHERE {where}) "
            "WHERE series IS NOT NULL GROUP BY ts, series"
        )
        if bucket_s:
            sql = f"SELECT MIN(ts) AS ts, series, AVG(value) AS value FROM ({per_tick}) GROUP BY series, strftime('%s', ts) / {int(bucket_s)}"
        else:
            sql = per_tick + " ORDER BY ts"
        return self.optional_rows(sql, (*case_args, *args))

    @staticmethod
    def merge_series(rows, found, bucket_s=None):
        """Adds sensor series to rows that share their timestamps (or their bucket)."""
        key = (lambda ts: epoch(ts) // bucket_s) if bucket_s else (lambda ts: ts)
        by_key = {key(r["ts"]): r for r in rows}
        for f in found:
            row = by_key.get(key(f["ts"]))
            if row is not None:
                row[f["series"]] = f["value"]

    def latest_sensors(self):
        """Every sensor of the newest tick that is under five minutes old."""
        return self.optional_rows(
            "SELECT kind, name, value FROM sensors WHERE ts = (SELECT MAX(ts) FROM sensors WHERE ts >= datetime('now', '-5 minutes')) "
            "ORDER BY kind, name"
        )

    # INDEXED BY: left alone, SQLite walks the (source, ref_id) index and sorts every matching row
    # (0.9 s over 3 million rows); the ts index reads the newest rows in order and stops at the limit.
    @staticmethod
    def _filter(severities, sources, text, since):
        """(where clauses, args) for the messages table, or None when a filter excludes everything."""
        where, args = [], []
        for column, values in (("severity", severities), ("source", sources)):
            if not values:
                return None
            where.append(f"{column} IN ({','.join('?' * len(values))})")
            args += sorted(values)
        if text:
            escaped = text.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")
            where.append("summary LIKE ? ESCAPE '\\'")
            args.append(f"%{escaped}%")
        if since:
            where.append("ts >= ?")
            args.append(since)
        return where, args

    def messages(self, severities, sources, text="", since=None, limit=300):
        found = self._filter(severities, sources, text, since)
        if found is None:
            return []
        where, args = found
        sql = (
            "SELECT source, ref_id, ts, severity, summary FROM messages INDEXED BY idx_messages_ts WHERE "
            + " AND ".join(where)
            + " ORDER BY ts DESC, rowid DESC LIMIT ?"
        )
        # fetch extra so that collapsing a burst of repeats still leaves a full page
        return collapse(self.rows(sql, (*args, limit * 4)))[:limit]

    def messages_page(self, severities, sources, text="", since=None, before=None, size=200):
        """One page of the list, newest first: `size` raw rows older than the `before` cursor,
        with repeats folded. `cursor` is where the next page starts (None when there is none).
        A burst of repeats that straddles two pages shows once on each."""
        found = self._filter(severities, sources, text, since)
        if found is None:
            return {"rows": [], "cursor": None}
        where, args = found
        if before:
            where.append("(ts < ? OR (ts = ? AND rowid < ?))")
            args += [before[0], before[0], before[1]]
        sql = (
            "SELECT source, ref_id, ts, severity, summary, rowid AS rid FROM messages INDEXED BY idx_messages_ts WHERE "
            + " AND ".join(where)
            + " ORDER BY ts DESC, rowid DESC LIMIT ?"
        )
        raw = self.rows(sql, (*args, size))
        more = len(raw) == size
        return {"rows": collapse(raw), "cursor": (raw[-1]["ts"], raw[-1]["rid"]) if more else None}

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
        sysstat = self.rows(
            "SELECT ts, cpu_pct, mem_pct, disk_io_kb, load_avg, gpu_pct, gpu_mem_pct, gpu_temp "
            f"FROM sysstat WHERE {SPAN} ORDER BY ts",
            bounds,
        )
        power = self.optional_rows(
            "SELECT ts, on_battery, battery_pct, cpu_watt, gpu_watt, battery_watt "
            f"FROM power WHERE {SPAN} ORDER BY ts",
            bounds,
        )
        found = self.sensor_series(SPAN, bounds)
        self.merge_series(sysstat, found)
        self.merge_series(power, found)
        return {
            "window": window,
            "messages": sorted(notable + nearby_info, key=lambda m: m["ts"]),
            "sysstat": sysstat,
            "power": power,
        }

    def detail(self, source, ref_id):
        if source not in TABLES:
            return {}
        found = self.rows(f"SELECT * FROM {source} WHERE {TABLES[source]} = ?", (ref_id,))
        return found[0] if found else {}

    def changes_before(self, ts, days=7, limit=5):
        """Package transactions in the days before a moment: what changed before it broke."""
        found = self.rows(
            "SELECT source, ref_id, ts, severity, summary FROM messages WHERE source = 'packages' "
            "AND ts BETWEEN datetime(?, ?) AND ? ORDER BY ts DESC",
            (ts, f"-{days} days", ts),
        )
        return found[:limit], len(found)

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
            "SELECT MIN(ts) AS ts, AVG(cpu_pct) AS cpu_pct, AVG(mem_pct) AS mem_pct, AVG(gpu_pct) AS gpu_pct, "
            "AVG(gpu_temp) AS gpu_temp "
            f"FROM sysstat WHERE ts >= datetime('now', ?) GROUP BY strftime('%s', ts) / {bucket_s} ORDER BY ts",
            (since,),
        )
        power = self.optional_rows(
            "SELECT MIN(ts) AS ts, AVG(cpu_watt) AS cpu_watt, AVG(gpu_watt) AS gpu_watt, "
            "AVG(battery_watt) AS battery_watt, AVG(battery_pct) AS battery_pct "
            f"FROM power WHERE ts >= datetime('now', ?) GROUP BY strftime('%s', ts) / {bucket_s} ORDER BY ts",
            (since,),
        )
        found = self.sensor_series("ts >= datetime('now', ?)", (since,), bucket_s)
        self.merge_series(load, found, bucket_s)
        self.merge_series(power, found, bucket_s)
        events = self.rows(
            "SELECT ts, severity, summary FROM messages WHERE severity != 'info' AND ts >= datetime('now', ?) "
            "ORDER BY ts DESC LIMIT 2000",
            (since,),
        )
        return {"hours": hours, "bucket_s": bucket_s, "load": load, "power": power, "events": events}

    def overview_page(self, sources):
        """Everything the overview shows, read in one go (it runs on the worker thread)."""
        return {
            "overview": self.overview(24),
            "latest": self.messages({"error", "warning"}, sources, limit=6),
            "gpu": self.last_gpu(),
            "power": self.last_power(),
            "sensors": self.latest_sensors(),
        }

    def last_power(self):
        """The newest power state: battery from the newest row, CPU watts from the newest row that
        has them (they are NULL while the RAPL counter is unreadable)."""
        newest = self.optional_rows(
            "SELECT ts, on_battery, battery_pct, battery_watt FROM power WHERE ts >= datetime('now', '-5 minutes') "
            "ORDER BY power_id DESC LIMIT 1"
        )
        cpu = self.optional_rows(
            "SELECT cpu_watt FROM power WHERE cpu_watt IS NOT NULL AND ts >= datetime('now', '-5 minutes') "
            "ORDER BY power_id DESC LIMIT 1"
        )
        if not newest:
            return None
        return {**newest[0], "cpu_watt": cpu[0]["cpu_watt"] if cpu else None}

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
