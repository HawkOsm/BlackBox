"""Turning timestamps, exit codes and field values into text."""
import re
import time
from datetime import datetime, timedelta, timezone
import data
from consts import KINDS, PRIORITIES, SIGNALS


def utc(ts):
    return datetime.strptime(ts, data.FMT).replace(tzinfo=timezone.utc)


def local(ts, fmt="%H:%M:%S"):
    return utc(ts).astimezone().strftime(fmt)


def local_epoch(t, fmt):
    return datetime.fromtimestamp(t).strftime(fmt)


def ago(ts):
    s = int(time.time() - data.epoch(ts))
    if s < 60:
        return "just now"
    if s < 3600:
        return f"{s // 60} min ago"
    if s < 86400:
        return f"{s // 3600} h ago"
    days = s // 86400
    return "yesterday" if days == 1 else f"{days} days ago"


def day_label(ts):
    day = utc(ts).astimezone().date()
    today = datetime.now().astimezone().date()
    if day == today:
        return "Today"
    if day == today - timedelta(days=1):
        return "Yesterday"
    return day.strftime("%A, %d %B")


def offset(seconds):
    sign = "+" if seconds > 0 else "−" if seconds < 0 else "±"
    s = abs(int(seconds))
    if s < 90:
        return f"{sign}{s} s"
    if s < 90 * 60:
        return f"{sign}{round(s / 60)} min"
    if s < 36 * 3600:
        return f"{sign}{round(s / 3600)} h"
    return f"{sign}{round(s / 86400)} days"


def describe_exit(code):
    sig = code & 0x7F
    if sig:
        return f"killed by {SIGNALS.get(sig, f'signal {sig}')}" + (", core dumped" if code & 0x80 else "")
    status = code >> 8
    return f"exited with status {status}" + (" (Rust panic)" if status == 101 else "")


def field_value(key, value):
    if value is None:
        return "–"
    if key == "exit_code":
        return f"{describe_exit(value)}  ·  raw {value}"
    if key == "priority" and isinstance(value, int) and 0 <= value < len(PRIORITIES):
        return f"{PRIORITIES[value]} ({value})"
    if key in ("cpu_pct", "mem_pct", "gpu_pct", "gpu_mem_pct"):
        return f"{value:.0f}%"
    if key == "gpu_temp":
        return f"{value:.0f} °C"
    if key == "disk_io_kb":
        return f"{value:.0f} KB/s"
    if key == "load_avg":
        return f"{value:.2f}"
    if key == "kind":
        return KINDS.get(value, value)
    return str(value)


def readable(summary):
    """Drops the long instance id of a templated unit: `systemd-coredump@13-3276…-0.service: x` -> `systemd-coredump: x`."""
    return re.sub(r"^([\w.-]+)@[^:\s]{12,}\.service: ", r"\1: ", summary)


CUSTOM = re.compile(r"^(\S+) \((pid \d+)(?:, child of ([^)]+))?\) (.*)$")
UNIT = re.compile(r"^([\w@.:\\-]+?): +(.*)$", re.S)
AUDIT = re.compile(r"^(\w+) (?:from|failed for \S+ via|hit by) (\S+)(.*)$")
AUDIT_WHAT = {"ANOM_ABEND": "crashed", "ANOM_PROMISCUOUS": "network card set to promiscuous mode",
              "USER_AUTH": "authentication failed"}


def split_summary(source, severity, summary):
    """(name, description, detail): the three parts of an event row. The name is the process or
    unit, the description what happened to it, the detail the reason or message."""
    summary = readable(summary)
    if source == "custom" and (m := CUSTOM.match(summary)):
        comm, pid, parent, what = m.groups()
        name = comm if comm != "?" else f"child of {parent}" if parent else "unknown process"
        where = f"{pid}, child of {parent}" if parent else pid
        if severity == "error" and what.startswith("killed by"):
            return name, "crashed", f"{what} · {where}"
        return name, what, where
    if source == "journald" and (m := UNIT.match(summary)):
        return m[1], "logged an error" if severity == "error" else "logged a warning" if severity == "warning" else "logged", m[2]
    if source == "auditd":
        if m := AUDIT.match(summary):
            kind, exe, rest = m.groups()
            if kind == "audit":
                return exe.rsplit("/", 1)[-1], "hit an audit rule", summary
            return exe.rsplit("/", 1)[-1], AUDIT_WHAT.get(kind, kind), f"{exe}{rest}"
        return "audit", "", summary
    if source == "packages" and (m := UNIT.match(summary)):
        what, _, rest = m[2].partition(" · ")
        return m[1], what, rest
    if source == "sysstat":
        part, _, rest = summary.partition(" ")
        return {"cpu": "CPU", "gpu": "GPU", "mem": "Memory"}.get(part, part), rest, "a system sample over its limit"
    if source == "boot":
        if summary.startswith("System started"):
            return "System started", "", summary.partition(", ")[2]
        return "Unclean shutdown", "", summary
    return summary, "", ""
