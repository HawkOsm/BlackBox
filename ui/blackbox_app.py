#!/usr/bin/env python3
"""Blackbox: native GTK4/libadwaita viewer for the blackbox database. Runs only while its window is open."""
import argparse
import bisect
import re
import sqlite3
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
gi.require_version("PangoCairo", "1.0")
from gi.repository import Adw, Gdk, Gio, GLib, GObject, Gtk, Pango, PangoCairo  # noqa: E402

import data  # noqa: E402

APP_ID = "io.github.hawkosm.Blackbox"
SOURCES = [("custom", "Processes"), ("journald", "Journal"), ("auditd", "Audit"), ("sysstat", "System")]
LABEL = dict(SOURCES)
SCOPES = [
    ("problems", "Problems", {"error", "warning"}),
    ("errors", "Errors", {"error"}),
    ("all", "Everything", {"error", "warning", "info"}),
]
RANGES = [("Last hour", 1), ("Last 6 hours", 6), ("Last 24 hours", 24), ("Last 7 days", 168), ("All time", None)]
WINDOWS = [("2m", "±2 min", 120), ("10m", "±10 min", 600), ("1h", "±1 h", 3600)]
ICON = {"error": "dialog-error-symbolic", "warning": "dialog-warning-symbolic", "info": "dialog-information-symbolic"}
TONE = {"error": "error", "warning": "warning", "info": "dim-label"}
MOMENT_ICON = "document-open-recent-symbolic"

# Chart series: one axis (percent), fixed order. Light and dark steps of the same three hues,
# validated for colour-blind separation against the card surface in both modes.
SERIES = {
    "cpu_pct": ("CPU", "#2a78d6", "#3987e5"),
    "mem_pct": ("Memory", "#eb6834", "#d95926"),
    "gpu_pct": ("GPU", "#1baf7a", "#199e70"),
}

FIELDS = {
    "comm": "Process",
    "exe": "Executable",
    "pid": "PID",
    "ppid": "Parent PID",
    "exit_code": "How it ended",
    "priority": "Priority",
    "unit": "Unit",
    "message": "Message",
    "event_type": "Audit event",
    "uid": "User ID",
    "executable": "Executable",
    "cpu_pct": "CPU",
    "mem_pct": "Memory",
    "disk_io_kb": "Disk I/O",
    "load_avg": "Load average",
    "gpu_pct": "GPU",
    "gpu_mem_pct": "GPU memory",
    "gpu_temp": "GPU temperature",
}
SIGNALS = {1: "SIGHUP", 2: "SIGINT", 3: "SIGQUIT", 4: "SIGILL", 5: "SIGTRAP", 6: "SIGABRT", 7: "SIGBUS",
           8: "SIGFPE", 9: "SIGKILL", 11: "SIGSEGV", 13: "SIGPIPE", 15: "SIGTERM", 31: "SIGSYS"}
PRIORITIES = ["emergency", "alert", "critical", "error", "warning", "notice", "info", "debug"]

CSS = """
.tile { padding: 14px 16px; }
.tile-value { font-size: 1.9em; font-weight: 800; font-feature-settings: "tnum"; }
.pill { border-radius: 999px; padding: 1px 8px; background: alpha(currentColor, 0.1);
        font-weight: 700; font-size: 0.8em; font-feature-settings: "tnum"; }
.day-header { font-weight: 700; font-size: 0.8em; opacity: 0.6; padding: 14px 12px 4px 12px; }
.event-title { font-weight: 600; }
.chart-card { padding: 14px 16px 10px 16px; }
.swatch { min-width: 10px; min-height: 10px; border-radius: 3px; }
.swatch-cpu_pct { background: #2a78d6; }
.swatch-mem_pct { background: #eb6834; }
.swatch-gpu_pct { background: #1baf7a; }
@media (prefers-color-scheme: dark) {
  .swatch-cpu_pct { background: #3987e5; }
  .swatch-mem_pct { background: #d95926; }
  .swatch-gpu_pct { background: #199e70; }
}
.mono { font-family: monospace; font-size: 0.85em; }
"""


# ---------------------------------------------------------------- time and text helpers

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
    return f"{sign}{s} s" if s < 90 else f"{sign}{round(s / 60)} min"


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
    return str(value)


def readable(summary):
    """Drops the long instance id of a templated unit: `systemd-coredump@13-3276…-0.service: x` -> `systemd-coredump: x`."""
    return re.sub(r"^([\w.-]+)@[^:\s]{12,}\.service: ", r"\1: ", summary)


def hex_rgb(h):
    return tuple(int(h[i:i + 2], 16) / 255 for i in (1, 3, 5))


def dark_mode():
    return Adw.StyleManager.get_default().get_dark()


# ---------------------------------------------------------------- chart

class Chart(Gtk.DrawingArea):
    """Percent-over-time lines on one 0-100 axis, with event ticks, a marked moment,
    direct labels at the line ends and a hover crosshair with a tooltip."""

    PAD_L, PAD_R, PAD_T, PAD_B = 40, 92, 10, 26

    def __init__(self, height=210):
        super().__init__(content_height=height, hexpand=True)
        self.t0 = self.t1 = 0
        self.series = []  # [(key, [(t, v)])]
        self.marker = None
        self.ticks = []  # [(t, text)]
        self.hover = None
        self.set_draw_func(self.draw)
        motion = Gtk.EventControllerMotion()
        motion.connect("motion", lambda _c, x, _y: self.set_hover(x))
        motion.connect("leave", lambda _c: self.set_hover(None))
        self.add_controller(motion)
        self.style_handler = Adw.StyleManager.get_default().connect("notify::dark", lambda *_: self.queue_draw())
        self.connect("unrealize", lambda *_: Adw.StyleManager.get_default().disconnect(self.style_handler))

    def set_data(self, t0, t1, rows, keys, marker=None, ticks=()):
        self.t0, self.t1, self.marker = t0, t1, marker
        self.series = []
        for key in keys:
            # a None is "not sampled this tick" (the GPU is read every third sample), not a gap
            pts = [(data.epoch(r["ts"]), r[key]) for r in rows if r.get(key) is not None]
            if pts:
                self.series.append((key, pts))
        self.ticks = sorted(ticks)
        self.queue_draw()

    def set_hover(self, x):
        self.hover = x
        self.queue_draw()

    @staticmethod
    def gap_limit(pts):
        steps = sorted(b[0] - a[0] for a, b in zip(pts, pts[1:]))
        return max(25, 3 * steps[len(steps) // 2]) if steps else 1e12

    def label(self, cr, text, x, y, rgba, align="left", bold=False):
        layout = self.create_pango_layout(None)
        weight = ' weight="bold"' if bold else ""
        layout.set_markup(f'<span size="small"{weight}>{GLib.markup_escape_text(text)}</span>', -1)
        w, h = layout.get_pixel_size()
        x -= {"left": 0, "center": w / 2, "right": w}[align]
        cr.set_source_rgba(*rgba)
        cr.move_to(x, y - h / 2)
        PangoCairo.show_layout(cr, layout)
        return w, h

    def draw(self, _area, cr, width, height):
        L, R, T, B = self.PAD_L, self.PAD_R, self.PAD_T, self.PAD_B
        pw, ph = width - L - R, height - T - B
        span = self.t1 - self.t0
        if pw < 60 or ph < 40 or span <= 0:
            return
        dark = dark_mode()
        fg = self.get_color()

        def ink(a):
            return (fg.red, fg.green, fg.blue, a)

        def X(t):
            return L + (t - self.t0) / span * pw

        def Y(v):
            return T + (1 - max(0.0, min(100.0, v)) / 100) * ph

        cr.set_line_width(1)
        for v in (0, 50, 100):
            y = round(Y(v)) + 0.5
            cr.set_source_rgba(*ink(0.14 if v == 0 else 0.07))
            cr.move_to(L, y)
            cr.line_to(L + pw, y)
            cr.stroke()
            self.label(cr, f"{v}%", L - 8, y, ink(0.55), "right")
        fmt = "%H:%M:%S" if span <= 900 else "%H:%M" if span <= 2 * 86400 else "%d %b"
        for frac, align in ((0, "left"), (0.5, "center"), (1, "right")):
            self.label(cr, local_epoch(self.t0 + frac * span, fmt), L + frac * pw, T + ph + 15, ink(0.55), align)

        for t, _ in self.ticks:
            if self.t0 <= t <= self.t1:
                x = round(X(t)) + 0.5
                cr.set_source_rgba(*ink(0.4))
                cr.move_to(x, T + ph)
                cr.line_to(x, T + ph - 7)
                cr.stroke()
        if self.marker is not None and self.t0 <= self.marker <= self.t1:
            x = round(X(self.marker)) + 0.5
            cr.set_source_rgba(*ink(0.6))
            cr.set_dash([4, 3])
            cr.move_to(x, T)
            cr.line_to(x, T + ph)
            cr.stroke()
            cr.set_dash([])

        cr.set_line_width(2)
        cr.set_line_join(1)  # round
        cr.set_line_cap(1)
        ends = []
        for key, pts in self.series:
            color = hex_rgb(SERIES[key][2 if dark else 1])
            gap = self.gap_limit(pts)
            cr.set_source_rgb(*color)
            prev = None
            for t, v in pts:
                if prev is None or t - prev > gap:
                    cr.move_to(X(t), Y(v))
                    cr.line_to(X(t) + 0.01, Y(v))  # a lone sample still shows as a dot
                else:
                    cr.line_to(X(t), Y(v))
                prev = t
            cr.stroke()
            t, v = pts[-1]
            ends.append([Y(v), key, v, X(t)])

        # direct labels at the line ends, nudged apart so they never collide
        ends.sort()
        for i in range(1, len(ends)):
            ends[i][0] = max(ends[i][0], ends[i - 1][0] + 15)
        overflow = (ends[-1][0] - (T + ph)) if ends else 0
        if overflow > 0:
            for e in ends:
                e[0] -= overflow
        for y, key, v, xend in ends:
            cr.set_source_rgb(*hex_rgb(SERIES[key][2 if dark else 1]))
            cr.arc(xend, Y(v), 3, 0, 6.2832)
            cr.fill()
            self.label(cr, f"{SERIES[key][0]} {v:.0f}%", min(xend + 8, L + pw + 8), y, ink(0.8))

        if self.hover is not None and L <= self.hover <= L + pw:
            self.draw_hover(cr, width, ink, dark, X, Y, L, T, pw, ph)

    def draw_hover(self, cr, width, ink, dark, X, Y, L, T, pw, ph):
        hx = self.hover
        t = self.t0 + (hx - L) / pw * (self.t1 - self.t0)
        cr.set_line_width(1)
        cr.set_source_rgba(*ink(0.3))
        cr.move_to(round(hx) + 0.5, T)
        cr.line_to(round(hx) + 0.5, T + ph)
        cr.stroke()
        surface = (0.21, 0.21, 0.23) if dark else (1, 1, 1)
        lines = [(None, local_epoch(t, "%H:%M:%S"))]
        for key, pts in self.series:
            times = [p[0] for p in pts]
            i = bisect.bisect_left(times, t)
            near = min((j for j in (i - 1, i) if 0 <= j < len(pts)), key=lambda j: abs(times[j] - t), default=None)
            if near is None or abs(times[near] - t) > self.gap_limit(pts):
                continue
            pt, pv = pts[near]
            color = hex_rgb(SERIES[key][2 if dark else 1])
            cr.set_source_rgb(*surface)
            cr.arc(X(pt), Y(pv), 6, 0, 6.2832)
            cr.fill()
            cr.set_source_rgb(*color)
            cr.arc(X(pt), Y(pv), 4, 0, 6.2832)
            cr.fill()
            lines.append((color, f"{SERIES[key][0]}  {pv:.0f}%"))
        near_ticks = [text for tt, text in self.ticks if abs(X(tt) - hx) <= 5]
        for text in near_ticks[:3]:
            lines.append(("event", text if len(text) <= 64 else text[:63] + "…"))

        layouts = []
        for color, text in lines:
            layout = self.create_pango_layout(None)
            weight = ' weight="bold"' if color is None else ""
            layout.set_markup(f'<span size="small"{weight}>{GLib.markup_escape_text(text)}</span>', -1)
            layouts.append((color, layout, *layout.get_pixel_size()))
        box_w = max(w for _, _, w, _ in layouts) + 34
        box_h = sum(h for *_, h in layouts) + 4 * (len(layouts) - 1) + 16
        bx = hx + 14 if hx + 14 + box_w < width else hx - 14 - box_w
        by = T + 4
        bg, text_rgba = ((0.96, 0.96, 0.97, 0.97), (0.1, 0.1, 0.12, 1)) if dark else ((0.14, 0.14, 0.16, 0.95), (1, 1, 1, 1))
        cr.set_source_rgba(*bg)
        r = 8
        cr.new_sub_path()
        cr.arc(bx + box_w - r, by + r, r, -1.5708, 0)
        cr.arc(bx + box_w - r, by + box_h - r, r, 0, 1.5708)
        cr.arc(bx + r, by + box_h - r, r, 1.5708, 3.1416)
        cr.arc(bx + r, by + r, r, 3.1416, 4.7124)
        cr.close_path()
        cr.fill()
        y = by + 8
        for color, layout, w, h in layouts:
            if isinstance(color, tuple):
                cr.set_source_rgb(*color)
                cr.rectangle(bx + 10, y + h / 2 - 4, 8, 8)
                cr.fill()
            elif color == "event":
                cr.set_source_rgba(*text_rgba[:3], 0.6)
                cr.rectangle(bx + 13, y + h / 2 - 4, 2, 8)
                cr.fill()
            cr.set_source_rgba(*text_rgba)
            cr.move_to(bx + (10 if color is None else 24), y)
            PangoCairo.show_layout(cr, layout)
            y += h + 4


def legend(keys):
    box = Gtk.Box(spacing=12, valign=Gtk.Align.CENTER)
    for key in keys:
        item = Gtk.Box(spacing=6)
        item.append(Gtk.Box(css_classes=["swatch", f"swatch-{key}"], valign=Gtk.Align.CENTER))
        item.append(Gtk.Label(label=SERIES[key][0], css_classes=["caption"]))
        box.append(item)
    return box


def samples_table(rows):
    fmt = lambda v, d=0: "–" if v is None else f"{v:.{d}f}"  # noqa: E731
    lines = [f"{'time':<10}{'cpu %':>7}{'mem %':>7}{'load':>7}{'gpu %':>7}{'gpu °C':>8}"]
    for s in rows:
        lines.append(
            f"{local(s['ts']):<10}{fmt(s['cpu_pct']):>7}{fmt(s['mem_pct']):>7}{fmt(s['load_avg'], 1):>7}"
            f"{fmt(s['gpu_pct']):>7}{fmt(s['gpu_temp']):>8}"
        )
    return "\n".join(lines)


def chart_card(title, chart, keys, extra=None, table=None):
    card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10, css_classes=["card", "chart-card"])
    head = Gtk.Box(spacing=12)
    head.append(Gtk.Label(label=title, xalign=0, hexpand=True, wrap=True, css_classes=["heading"]))
    if extra is not None:
        head.append(extra)
    card.append(head)
    card.append(chart)
    if len(keys) > 1:
        card.append(legend(keys))
    if table:
        expander = Gtk.Expander(label="Show as a table")
        expander.set_child(Gtk.Label(label=table, xalign=0, selectable=True, css_classes=["mono"], margin_top=6))
        card.append(expander)
    return card


def tile(caption, value, detail=None, icon=None, tone=None):
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2, css_classes=["card", "tile"])
    head = Gtk.Box(spacing=6)
    if icon:
        head.append(Gtk.Image(icon_name=icon, css_classes=[tone] if tone else []))
    head.append(Gtk.Label(label=caption, xalign=0, css_classes=["caption-heading", "dim-label"]))
    box.append(head)
    box.append(Gtk.Label(label=value, xalign=0, css_classes=["tile-value"]))
    box.append(Gtk.Label(label=detail or " ", xalign=0, css_classes=["caption", "dim-label"]))
    return box


def event_icon(m, size=16):
    if m.get("moment"):
        return Gtk.Image(icon_name=MOMENT_ICON, pixel_size=size, css_classes=["dim-label"])
    return Gtk.Image(icon_name=ICON.get(m["severity"], ICON["info"]), pixel_size=size, css_classes=[TONE.get(m["severity"], "dim-label")])


class EventRow(Gtk.ListBoxRow):
    def __init__(self, m):
        super().__init__()
        self.event = m
        box = Gtk.Box(spacing=12, margin_top=8, margin_bottom=8, margin_start=6, margin_end=6)
        icon = event_icon(m)
        icon.set_valign(Gtk.Align.START)
        icon.set_margin_top(2)
        box.append(icon)
        text = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=3, hexpand=True)
        text.append(Gtk.Label(
            label=readable(m["summary"]), xalign=0, wrap=True, wrap_mode=Pango.WrapMode.WORD_CHAR, lines=2,
            ellipsize=Pango.EllipsizeMode.END, max_width_chars=50, css_classes=["event-title"],
        ))
        meta = f"{LABEL.get(m['source'], m['source'])} · {local(m['ts'], '%H:%M')} · {ago(m['ts'])}"
        text.append(Gtk.Label(label=meta, xalign=0, css_classes=["caption", "dim-label"]))
        box.append(text)
        if m.get("count", 1) > 1:
            box.append(Gtk.Label(
                label=f"×{m['count']}", css_classes=["pill"], valign=Gtk.Align.CENTER,
                tooltip_text=f"Repeated {m['count']} times since {local(m['first_ts'], '%b %d %H:%M:%S')}",
            ))
        self.set_child(box)


# ---------------------------------------------------------------- window

class Window(Adw.ApplicationWindow):
    def __init__(self, app, db):
        super().__init__(application=app, title="Blackbox")
        self.set_default_size(1280, 820)
        self.set_size_request(360, 480)
        self.db = db
        self.sources = {key for key, _ in SOURCES}
        self.hours = 24
        self.selected = None
        self.shown_key = None
        self.current = None
        self.last_ctx = None
        self.last_detail = {}
        self.window_secs = 120
        self.overview_at = 0

        self.split = Adw.NavigationSplitView(min_sidebar_width=340, max_sidebar_width=470, sidebar_width_fraction=0.36)
        self.split.set_sidebar(self.build_sidebar())
        self.split.set_content(self.build_content())
        self.toasts = Adw.ToastOverlay(child=self.split)
        self.set_content(self.toasts)

        narrow = Adw.Breakpoint.new(Adw.BreakpointCondition.parse("max-width: 720sp"))
        narrow.add_setter(self.split, "collapsed", True)
        self.add_breakpoint(narrow)

        self.refresh(force=True)
        GLib.timeout_add_seconds(5, self.refresh)

    # ---- sidebar: the event list

    def build_sidebar(self):
        view = Adw.ToolbarView()
        header = Adw.HeaderBar()
        self.title = Adw.WindowTitle(title="Blackbox", subtitle="Connecting…")
        header.set_title_widget(self.title)
        search_button = Gtk.ToggleButton(icon_name="system-search-symbolic", tooltip_text="Search (type to start)")
        header.pack_start(search_button)
        self.search_button = search_button
        header.pack_end(Gtk.MenuButton(icon_name="view-more-symbolic", tooltip_text="Sources and time range",
                                       popover=self.build_filters()))
        view.add_top_bar(header)

        self.search = Gtk.SearchEntry(placeholder_text="Search events", hexpand=True)
        self.search.connect("search-changed", lambda *_: self.refresh(force=True))
        bar = Gtk.SearchBar(child=self.search, key_capture_widget=self)
        bar.connect_entry(self.search)
        search_button.bind_property("active", bar, "search-mode-enabled", GObject.BindingFlags.BIDIRECTIONAL)
        view.add_top_bar(bar)

        self.scope = Adw.ToggleGroup(homogeneous=True, margin_start=12, margin_end=12, margin_top=6, margin_bottom=6)
        for name, label, _ in SCOPES:
            self.scope.add(Adw.Toggle(name=name, label=label))
        self.scope.set_active_name("problems")
        self.scope.connect("notify::active-name", lambda *_: self.refresh(force=True))
        view.add_top_bar(self.scope)

        self.list = Gtk.ListBox(selection_mode=Gtk.SelectionMode.SINGLE, css_classes=["navigation-sidebar"])
        self.list.set_header_func(self.day_header)
        self.list.connect("row-activated", lambda _lb, row: self.show_detail(row.event))
        self.list.set_placeholder(Adw.StatusPage(
            icon_name="object-select-symbolic", title="All quiet",
            description="Nothing matches these filters. Choose Everything to see all that was recorded.",
        ))
        view.set_content(Gtk.ScrolledWindow(child=self.list, vexpand=True, hscrollbar_policy=Gtk.PolicyType.NEVER))

        self.footer = Gtk.Label(css_classes=["caption", "dim-label"], margin_top=8, margin_bottom=8, ellipsize=Pango.EllipsizeMode.END)
        view.add_bottom_bar(self.footer)
        return Adw.NavigationPage(title="Blackbox", tag="events", child=view)

    def build_filters(self):
        group = Gtk.ListBox(css_classes=["boxed-list"], selection_mode=Gtk.SelectionMode.NONE)
        for key, label in SOURCES:
            row = Adw.SwitchRow(title=label, active=True)
            row.connect("notify::active", self.on_source, key)
            group.append(row)
        ranges = Gtk.ListBox(css_classes=["boxed-list"], selection_mode=Gtk.SelectionMode.NONE)
        combo = Adw.ComboRow(title="Time range", model=Gtk.StringList.new([label for label, _ in RANGES]))
        combo.set_selected(2)
        combo.connect("notify::selected", self.on_range)
        ranges.append(combo)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10, width_request=280,
                      margin_top=6, margin_bottom=6, margin_start=6, margin_end=6)
        box.append(Gtk.Label(label="Sources", xalign=0, css_classes=["heading"]))
        box.append(group)
        box.append(ranges)
        return Gtk.Popover(child=box)

    def day_header(self, row, before):
        day = day_label(row.event["ts"])
        if before is None or day_label(before.event["ts"]) != day:
            row.set_header(Gtk.Label(label=day, xalign=0, css_classes=["day-header"]))
        else:
            row.set_header(None)

    def on_source(self, row, _param, key):
        (self.sources.add if row.get_active() else self.sources.discard)(key)
        self.refresh(force=True)

    def on_range(self, combo, _param):
        self.hours = RANGES[combo.get_selected()][1]
        self.refresh(force=True)

    # ---- content: overview or one event

    def build_content(self):
        view = Adw.ToolbarView()
        header = Adw.HeaderBar()
        self.copy_button = Gtk.Button(icon_name="edit-copy-symbolic", tooltip_text="Copy a text report of this moment", visible=False)
        self.copy_button.connect("clicked", lambda *_: self.copy_report())
        header.pack_end(self.copy_button)
        header.pack_end(Gtk.MenuButton(icon_name=MOMENT_ICON, tooltip_text="Go to a time", popover=self.build_time_picker()))
        self.overview_button = Gtk.Button(icon_name="go-previous-symbolic", tooltip_text="Back to the overview", visible=False)
        self.overview_button.connect("clicked", lambda *_: self.show_overview())
        header.pack_start(self.overview_button)
        view.add_top_bar(header)
        self.banner = Adw.Banner(revealed=False)
        view.add_top_bar(self.banner)

        self.stack = Gtk.Stack(transition_type=Gtk.StackTransitionType.CROSSFADE)
        self.overview_scroll = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER)
        self.detail_scroll = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER)
        self.stack.add_named(self.overview_scroll, "overview")
        self.stack.add_named(self.detail_scroll, "detail")
        view.set_content(self.stack)
        self.content_page = Adw.NavigationPage(title="Overview", tag="detail", child=view)
        return self.content_page

    def build_time_picker(self):
        now = datetime.now()
        calendar = Gtk.Calendar()
        hour = Gtk.SpinButton.new_with_range(0, 23, 1)
        minute = Gtk.SpinButton.new_with_range(0, 59, 1)
        hour.set_value(now.hour)
        minute.set_value(now.minute)
        clock = Gtk.Box(spacing=6, halign=Gtk.Align.CENTER)
        clock.append(hour)
        clock.append(Gtk.Label(label=":"))
        clock.append(minute)
        button = Gtk.Button(label="Show this moment", css_classes=["suggested-action", "pill"])
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12, margin_top=6, margin_bottom=6, margin_start=6, margin_end=6)
        box.append(Gtk.Label(label="What happened at…", xalign=0, css_classes=["heading"]))
        box.append(calendar)
        box.append(clock)
        box.append(button)
        popover = Gtk.Popover(child=box)

        def go(*_):
            d = calendar.get_date()
            moment = datetime(d.get_year(), d.get_month(), d.get_day_of_month(), hour.get_value_as_int(), minute.get_value_as_int())
            ts = moment.astimezone(timezone.utc).strftime(data.FMT)
            popover.popdown()
            self.show_detail({
                "moment": True, "source": None, "ref_id": None, "ts": ts, "severity": "info",
                "summary": f"What happened around {moment.strftime('%a %d %b, %H:%M')}",
            })

        button.connect("clicked", go)
        return popover

    def page(self, child):
        clamp = Adw.Clamp(maximum_size=920, tightening_threshold=600, child=child)
        return clamp

    def show_overview(self):
        self.current = None
        self.selected = None
        self.list.unselect_all()
        self.copy_button.set_visible(False)
        self.overview_button.set_visible(False)
        self.content_page.set_title("Overview")
        self.render_overview()
        self.stack.set_visible_child_name("overview")

    def render_overview(self):
        try:
            s = self.db.summary()
            ov = self.db.overview(24)
            latest = self.db.messages({"error", "warning"}, self.sources, limit=6)
        except (sqlite3.Error, OSError):
            return
        self.overview_at = time.monotonic()
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=20, margin_top=24, margin_bottom=24, margin_start=18, margin_end=18)
        head = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        head.append(Gtk.Label(label="Last 24 hours", xalign=0, css_classes=["title-1"]))
        head.append(Gtk.Label(label="Pick an event, or go to any moment with the clock button.", xalign=0, wrap=True, css_classes=["dim-label"]))
        box.append(head)

        tiles = Gtk.FlowBox(selection_mode=Gtk.SelectionMode.NONE, homogeneous=True, min_children_per_line=2,
                            max_children_per_line=3, column_spacing=12, row_spacing=12)
        last = s["last_sample"] or {}
        pct = lambda v: "–" if v is None else f"{v:.0f}%"  # noqa: E731
        tiles.append(tile("Errors", str(s["errors_24h"]), "last 24 h", ICON["error"], "error"))
        tiles.append(tile("Warnings", str(s["warnings_24h"]), "last 24 h", ICON["warning"], "warning"))
        tiles.append(tile("CPU", pct(last.get("cpu_pct")), f"load {last['load_avg']:.1f}" if last.get("load_avg") is not None else None))
        tiles.append(tile("Memory", pct(last.get("mem_pct")), "in use"))
        gpu = self.db.last_gpu()
        tiles.append(tile("GPU", pct(gpu and gpu["gpu_pct"]), f"{gpu['gpu_temp']:.0f} °C" if gpu and gpu["gpu_temp"] is not None else "asleep or absent"))
        size = s["db_bytes"] / 1e6
        tiles.append(tile("Recorded", f"{size:.0f} MB" if size >= 10 else f"{size:.1f} MB", "on disk"))
        for child in list(tiles):
            child.set_focusable(False)
        box.append(tiles)

        now = time.time()
        chart = Chart(200)
        keys = [k for k in SERIES if any(r.get(k) is not None for r in ov["load"])]
        start = max(now - 86400, data.epoch(ov["load"][0]["ts"])) if ov["load"] else now - 86400
        ticks = [(data.epoch(e["ts"]), readable(e["summary"])) for e in ov["events"]]
        chart.set_data(start, now, ov["load"], keys, ticks=ticks)
        every = ov["bucket_s"]
        per = f"{every // 60}-minute" if every >= 60 else f"{every}-second"
        title = "System load, last 24 hours" if now - start > 86000 else f"System load since {local_epoch(start, '%H:%M')}"
        box.append(chart_card(f"{title} · {per} averages", chart, keys))

        group = Adw.PreferencesGroup(title="Latest problems")
        if not latest:
            group.set_description("Nothing to report.")
        for m in latest:
            row = Adw.ActionRow(title=readable(m["summary"]), subtitle=f"{LABEL.get(m['source'], m['source'])} · {ago(m['ts'])}",
                                use_markup=False, activatable=True, title_lines=2)
            row.add_prefix(event_icon(m))
            if m["count"] > 1:
                row.add_suffix(Gtk.Label(label=f"×{m['count']}", css_classes=["pill"], valign=Gtk.Align.CENTER))
            row.add_suffix(Gtk.Image(icon_name="go-next-symbolic", css_classes=["dim-label"]))
            row.connect("activated", lambda _r, e=m: self.show_detail(e))
            group.add(row)
        box.append(group)
        self.overview_scroll.set_child(self.page(box))

    def show_detail(self, m):
        self.current = m
        self.selected = None if m.get("moment") else (m["source"], m["ref_id"])
        self.sync_selection()
        try:
            content = self.build_detail(m)
        except (sqlite3.Error, OSError):
            self.toasts.add_toast(Adw.Toast(title="Cannot read the database"))
            return
        self.detail_scroll.set_child(self.page(content))
        self.detail_scroll.get_vadjustment().set_value(0)
        self.content_page.set_title("A moment" if m.get("moment") else LABEL.get(m["source"], "Event"))
        self.copy_button.set_visible(True)
        self.overview_button.set_visible(not self.split.get_collapsed())
        self.stack.set_visible_child_name("detail")
        self.split.set_show_content(True)

    def build_detail(self, m):
        ctx = self.db.context(m["ts"], self.window_secs)
        detail = {} if m.get("moment") else self.db.detail(m["source"], m["ref_id"])
        self.last_ctx, self.last_detail = ctx, detail
        t = data.epoch(m["ts"])
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=22, margin_top=24, margin_bottom=24, margin_start=18, margin_end=18)

        hero = Gtk.Box(spacing=16)
        icon = event_icon(m, 40)
        icon.set_valign(Gtk.Align.START)
        hero.append(icon)
        col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, hexpand=True)
        col.append(Gtk.Label(label=readable(m["summary"]), xalign=0, wrap=True, wrap_mode=Pango.WrapMode.WORD_CHAR, selectable=True, css_classes=["title-2"]))
        where = "Picked moment" if m.get("moment") else LABEL.get(m["source"], m["source"])
        col.append(Gtk.Label(label=f"{where} · {local(m['ts'], '%a %d %b, %H:%M:%S')} · {ago(m['ts'])}", xalign=0, wrap=True, css_classes=["dim-label"]))
        if m.get("count", 1) > 1:
            col.append(Gtk.Label(label=f"Repeated {m['count']} times since {local(m['first_ts'], '%H:%M:%S')}",
                                 xalign=0, wrap=True, halign=Gtk.Align.START, css_classes=["pill"]))
        hero.append(col)
        box.append(hero)

        windows = Adw.ToggleGroup(valign=Gtk.Align.CENTER)
        for name, label, _ in WINDOWS:
            windows.add(Adw.Toggle(name=name, label=label))
        windows.set_active_name(next(n for n, _, s in WINDOWS if s == self.window_secs))
        windows.connect("notify::active-name", self.on_window)
        keys = [k for k in SERIES if any(r.get(k) is not None for r in ctx["sysstat"])]
        if ctx["sysstat"]:
            chart = Chart(220)
            ticks = [(data.epoch(e["ts"]), e["summary"]) for e in ctx["messages"] if (e["source"], e["ref_id"]) != self.selected]
            chart.set_data(t - self.window_secs, t + self.window_secs, ctx["sysstat"], keys, marker=t, ticks=ticks)
            body, table = chart, samples_table(ctx["sysstat"])
        else:
            body, table = Gtk.Label(label="No system samples in this window. Was the collector running?", xalign=0,
                                    css_classes=["dim-label"], margin_top=8, margin_bottom=8), None
        box.append(chart_card("System around this moment", body, keys, extra=windows, table=table))

        if m.get("source") == "custom" and detail.get("exit_code", 0) & 0x7F:
            trace = self.db.coredump(detail["pid"], m["ts"])
            if trace:
                group = Adw.PreferencesGroup(title="Core dump", description="From systemd-coredump. Full dump: coredumpctl info " + str(detail["pid"]))
                expander = Gtk.Expander(label="Stack trace", expanded=True)
                expander.set_child(Gtk.Label(label=trace, xalign=0, wrap=True, wrap_mode=Pango.WrapMode.CHAR, selectable=True,
                                             css_classes=["mono"], margin_top=6))
                card = Gtk.Box(css_classes=["card", "chart-card"])
                card.append(expander)
                group.add(card)
                box.append(group)

        if detail:
            group = Adw.PreferencesGroup(title="Details")
            for key, value in detail.items():
                if key.endswith("_id") or key == "ts":
                    continue
                row = Adw.ActionRow(title=FIELDS.get(key, key.replace("_", " ").capitalize()), subtitle=field_value(key, value),
                                    use_markup=False, css_classes=["property"])
                row.set_subtitle_selectable(True)
                group.add(row)
            box.append(group)

        label = next(lbl for _, lbl, s in WINDOWS if s == self.window_secs)
        around = Adw.PreferencesGroup(title="Around this moment", description=f"{len(ctx['messages'])} events within {label}")
        for other in ctx["messages"]:
            same = (other["source"], other["ref_id"]) == self.selected
            row = Adw.ActionRow(title=readable(other["summary"]), title_lines=2, use_markup=False, activatable=not same,
                                subtitle=f"{offset(data.epoch(other['ts']) - t)} · {LABEL.get(other['source'], other['source'])}")
            row.add_prefix(event_icon(other))
            if same:
                row.add_suffix(Gtk.Label(label="This event", css_classes=["pill"], valign=Gtk.Align.CENTER))
            else:
                row.connect("activated", lambda _r, e=other: self.show_detail(e))
            around.add(row)
        box.append(around)
        return box

    def on_window(self, group, _param):
        self.window_secs = next(s for n, _, s in WINDOWS if n == group.get_active_name())
        if self.current is not None:
            GLib.idle_add(lambda: self.show_detail(self.current) and False)

    def copy_report(self):
        m, ctx = self.current, self.last_ctx
        if m is None or ctx is None:
            return
        t = data.epoch(m["ts"])
        where = "picked moment" if m.get("moment") else LABEL.get(m["source"], m["source"])
        out = [f"Blackbox: {m['summary']}", f"{where}, {local(m['ts'], '%Y-%m-%d %H:%M:%S %Z')}"]
        if m.get("count", 1) > 1:
            out.append(f"repeated {m['count']} times since {local(m['first_ts'])}")
        if self.last_detail:
            out += ["", "Details"]
            out += [f"  {FIELDS.get(k, k)}: {field_value(k, v)}" for k, v in self.last_detail.items() if not k.endswith("_id") and k != "ts"]
        if ctx["sysstat"]:
            out += ["", f"System, ±{self.window_secs // 60} min"] + ["  " + line for line in samples_table(ctx["sysstat"]).splitlines()]
        out += ["", "Events around it"]
        out += [f"  {offset(data.epoch(e['ts']) - t):>8}  {e['severity']:<7} {LABEL.get(e['source'], e['source']):<8} {e['summary']}" for e in ctx["messages"]]
        Gdk.Display.get_default().get_clipboard().set("\n".join(out) + "\n")
        self.toasts.add_toast(Adw.Toast(title="Report copied to the clipboard", timeout=2))

    # ---- refresh

    def since(self):
        if self.hours is None:
            return None
        return (datetime.now(timezone.utc) - timedelta(hours=self.hours)).strftime(data.FMT)

    def refresh(self, force=False):
        severities = next(sev for name, _, sev in SCOPES if name == self.scope.get_active_name())
        try:
            rows = self.db.messages(severities, self.sources, self.search.get_text(), self.since())
            summary = self.db.summary()
        except (sqlite3.Error, OSError):
            self.title.set_subtitle("Cannot read the database")
            self.banner.set_title("Cannot read the database. Is the collector installed and running?")
            self.banner.set_revealed(True)
            return True
        age = summary["sample_age_s"]
        if age is not None and age < 45:
            self.title.set_subtitle("Recording")
            self.banner.set_revealed(False)
        elif age is None:
            self.title.set_subtitle("No data yet")
            self.banner.set_revealed(False)
        else:
            self.title.set_subtitle("Collector stopped?")
            self.banner.set_title(f"Nothing recorded for {max(age // 60, 1)} min. Is the collector running?")
            self.banner.set_revealed(True)
        shown = sum(r["count"] for r in rows)
        self.footer.set_label(f"{shown} events  ·  {summary['errors_24h']} errors, {summary['warnings_24h']} warnings in 24 h  ·  {summary['db_bytes'] / 1e6:.1f} MB")

        # rebuilt when the rows change, and once a minute so "3 min ago" stays true
        key = (int(time.time() // 60), tuple((r["source"], r["ref_id"], r["summary"], r["count"]) for r in rows))
        if force or key != self.shown_key:
            self.shown_key = key
            self.list.remove_all()
            for m in rows:
                self.list.append(EventRow(m))
            self.sync_selection()
        if self.current is None and (force or time.monotonic() - self.overview_at > 30):
            self.render_overview()
        return True

    def sync_selection(self):
        if self.selected is None:
            self.list.unselect_all()
            return
        index = 0
        while (row := self.list.get_row_at_index(index)) is not None:
            if (row.event["source"], row.event["ref_id"]) == self.selected:
                self.list.select_row(row)
                return
            index += 1
        self.list.unselect_all()

    # ---- testing

    def snapshot(self, path, select):
        row = self.list.get_row_at_index(select) if select is not None else None
        if row is not None:
            self.show_detail(row.event)
        GLib.timeout_add(900, self.write_png, path)

    def write_png(self, path):
        paintable = Gtk.WidgetPaintable.new(self)
        snap = Gtk.Snapshot()
        paintable.snapshot(snap, self.get_width(), self.get_height())
        texture = self.get_native().get_renderer().render_texture(snap.to_node(), None)
        texture.save_to_png(path)
        self.get_application().quit()
        return False


class App(Adw.Application):
    def __init__(self, args):
        flags = Gio.ApplicationFlags.NON_UNIQUE if args.screenshot else Gio.ApplicationFlags.DEFAULT_FLAGS
        super().__init__(application_id=APP_ID, flags=flags)
        self.args = args

    def do_startup(self):
        Adw.Application.do_startup(self)
        provider = Gtk.CssProvider()
        provider.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        if self.args.style:
            scheme = Adw.ColorScheme.FORCE_DARK if self.args.style == "dark" else Adw.ColorScheme.FORCE_LIGHT
            Adw.StyleManager.get_default().set_color_scheme(scheme)

    def do_activate(self):
        win = self.props.active_window
        if win is None:
            win = Window(self, data.Data(self.args.db))
            if self.args.size:
                w, h = (int(v) for v in self.args.size.lower().split("x"))
                win.set_default_size(w, h)
            if self.args.screenshot:
                select = 0 if self.args.select_first else self.args.select
                GLib.timeout_add(1500, lambda: win.snapshot(self.args.screenshot, select) or False)
            if self.args.close_after:
                GLib.timeout_add_seconds(self.args.close_after, lambda: win.close() or False)
            if self.args.screenshot:
                # a headless display has no frame clock, so transitions would never finish
                Gtk.Settings.get_default().set_property("gtk-enable-animations", False)
            # otherwise the first row takes the initial focus, and a focused row gets selected
            win.set_focus(win.search_button)
        win.present()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", default=data.default_db())
    parser.add_argument("--screenshot", help="write a PNG of the window and exit (for testing)")
    parser.add_argument("--close-after", type=int, help="close the window after N seconds (for testing)")
    parser.add_argument("--select-first", action="store_true", help="with --screenshot: open the newest entry first")
    parser.add_argument("--select", type=int, help="with --screenshot: open the entry at this list position")
    parser.add_argument("--style", choices=["light", "dark"], help="force a colour scheme (for testing)")
    parser.add_argument("--size", help="initial window size, e.g. 480x800 (for testing)")
    args = parser.parse_args()
    sys.exit(App(args).run([sys.argv[0]]))


if __name__ == "__main__":
    main()
