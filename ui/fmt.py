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
