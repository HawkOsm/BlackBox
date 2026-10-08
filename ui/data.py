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


def group_key(r):
    """What makes two events repeats of one another: same source, severity and text bar a pid."""
    return r["source"], r["severity"], PID.sub("pid", r["summary"])


def collapse(rows, window=600):
    """Folds repeats of the same message into one row with a count (rows arrive newest first).
    Messages that differ only in a pid count as the same, and a repeat joins its group even with
    other events in between, as long as it is within `window` seconds of the group's last one:
    a login that runs the same failing helper 90 times shows up as one row, not 90."""
    out, open_groups = [], {}
    for r in rows:
        key = group_key(r)
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

    def glance(self, hours, sources, bars=48):
        """Everything the overview chart draws for the last `hours` (None: since the first
        sample): load and power averaged into about 150 buckets, error and warning counts in
        `bars` equal slices, and the error-level boots and crashes that get a marker."""
        now = time.time()
        if hours is None:
            first = self.rows("SELECT MIN(ts) AS ts FROM sysstat")[0]["ts"]
            t0 = epoch(first) if first else now - 86400
        else:
            t0 = now - hours * 3600
        since = time.strftime(FMT, time.gmtime(t0))
        bucket_s = max(10, int((now - t0) / 150) // 10 * 10)
        load = self.rows(
            "SELECT MIN(ts) AS ts, AVG(cpu_pct) AS cpu_pct, AVG(mem_pct) AS mem_pct, AVG(gpu_pct) AS gpu_pct, "
            "AVG(gpu_temp) AS gpu_temp "
            f"FROM sysstat WHERE ts >= ? GROUP BY strftime('%s', ts) / {bucket_s} ORDER BY ts",
            (since,),
        )
        power = self.optional_rows(
            "SELECT MIN(ts) AS ts, AVG(cpu_watt) AS cpu_watt, AVG(gpu_watt) AS gpu_watt, "
            "AVG(battery_watt) AS battery_watt, AVG(battery_pct) AS battery_pct "
            f"FROM power WHERE ts >= ? GROUP BY strftime('%s', ts) / {bucket_s} ORDER BY ts",
            (since,),
        )
        found = self.sensor_series("ts >= ?", (since,), bucket_s)
        self.merge_series(load, found, bucket_s)
        self.merge_series(power, found, bucket_s)
        counts = [{"error": 0, "warning": 0} for _ in range(bars)]
        markers = []
        if sources:
            src = f"source IN ({','.join('?' * len(sources))})"
            bar_s = (now - t0) / bars
            for r in self.rows(
                f"SELECT CAST((strftime('%s', ts) - ?) / ? AS INTEGER) AS i, severity, COUNT(*) AS n FROM messages "
                f"WHERE severity IN ('error', 'warning') AND ts >= ? AND {src} GROUP BY i, severity",
                (int(t0), bar_s, since, *sorted(sources)),
            ):
                counts[min(bars - 1, max(0, r["i"]))][r["severity"]] += r["n"]
            markers = self.rows(
                f"SELECT ts, summary FROM messages WHERE severity = 'error' AND source IN ('boot', 'custom') "
                f"AND ts >= ? AND {src} ORDER BY ts DESC LIMIT 60",
                (since, *sorted(sources)),
            )
        return {"t0": t0, "t1": now, "bucket_s": bucket_s, "load": load, "power": power,
                "bars": counts, "markers": markers}

    def overview_page(self, hours, sources):
        """Everything the overview shows, read in one go (it runs on the worker thread)."""
        return {"glance": self.glance(hours, sources), "crashes": self.crashes()}

    def crashes(self):
        """{process name: (crash count, first ts)}: tags that tie a running process to the event list."""
        out = {}
        for r in self.rows(
            "SELECT c.comm AS name, COUNT(*) AS n, MIN(c.ts) AS first FROM messages m JOIN custom c ON c.custom_id = m.ref_id "
            "WHERE m.source = 'custom' AND m.severity = 'error' AND c.comm IS NOT NULL AND c.comm != '?' GROUP BY c.comm"
        ) + self.rows(
            "SELECT a.executable AS name, COUNT(*) AS n, MIN(a.ts) AS first FROM messages m JOIN auditd a ON a.audit_id = m.ref_id "
            "WHERE m.source = 'auditd' AND m.severity = 'error' AND a.event_type = 'ANOM_ABEND' AND a.executable IS NOT NULL "
            "GROUP BY a.executable"
        ):
            name = r["name"].rsplit("/", 1)[-1][:15]  # an executable's name as /proc/<pid>/comm has it
            n, first = out.get(name, (0, r["first"]))
            out[name] = (n + r["n"], min(first, r["first"]))
        return out

    def counts(self, sources, text, since):
        """Events per severity under the current sources, search and range: the scope switch's numbers."""
        found = self._filter({"error", "warning", "info"}, sources, text, since)
        if found is None:
            return {}
        where, args = found
        rows = self.rows(f"SELECT severity, COUNT(*) AS n FROM messages WHERE {' AND '.join(where)} GROUP BY severity", args)
        return {r["severity"]: r["n"] for r in rows}

    def hourly(self, severities, sources, text):
        """{group key: [24 counts]}, oldest hour first: the last day of each folded row, for its sparkline."""
        found = self._filter(severities, sources, text, time.strftime(FMT, time.gmtime(time.time() - 86400)))
        if found is None:
            return {}
        where, args = found
        now = time.time()
        out = {}
        for r in self.rows(f"SELECT source, severity, summary, ts FROM messages WHERE {' AND '.join(where)} LIMIT 200000", args):
            hour = 23 - int((now - epoch(r["ts"])) // 3600)
            if 0 <= hour < 24:
                out.setdefault(group_key(r), [0] * 24)[hour] += 1
        return out

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
