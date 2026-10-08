"""Small reusable widgets: chart cards, event rows and their sparklines."""
from gtkenv import Gtk, Pango
from chart import Chart, hex_rgb, legend
from consts import ICON, LABEL, MOMENT_ICON, TONE
from fmt import ago, local, readable, split_summary

SEVERITY_RGB = {"error": "#EF4444", "warning": "#F2B33D", "info": "#D6D9E9"}
EMPTY_HOUR = hex_rgb("#3A3A49")


def rows_table(spec, rows):
    """Monospace text table of the samples one chart draws, without rows that have none of its values."""
    cols = spec["columns"]
    fmt = lambda v, d: "–" if v is None else f"{v:.{d}f}"  # noqa: E731
    lines = [f"{'time':<10}" + "".join(f"{name:>8}" for name, _, _ in cols)]
    for r in rows:
        if all(r.get(key) is None for _, key, _ in cols):
            continue
        lines.append(f"{local(r['ts']):<10}" + "".join(f"{fmt(r.get(key), d):>8}" for _, key, d in cols))
    return "\n".join(lines)


def spec_card(spec, title, rows, t0, t1, ticks=(), marker=None, height=200, extra=None):
    """One chart card for a CHARTS entry; a note instead of an empty chart when nothing was recorded."""
    keys = [k for k in spec["series"] if any(r.get(k) is not None for r in rows)]
    if keys:
        chart = Chart(height, spec["unit"], spec["ymax"])
        chart.set_data(t0, t1, rows, keys, marker=marker, ticks=ticks)
        body, table = chart, rows_table(spec, rows)
    else:
        body = Gtk.Label(label=spec["empty"], xalign=0, wrap=True, css_classes=["dim-label"], margin_top=8, margin_bottom=8)
        table = None
    return chart_card(title, body, keys, extra=extra, table=table)


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


def event_icon(m, size=16):
    if m.get("moment"):
        return Gtk.Image(icon_name=MOMENT_ICON, pixel_size=size, css_classes=["dim-label"])
    return Gtk.Image(icon_name=ICON.get(m["severity"], ICON["info"]), pixel_size=size, css_classes=[TONE.get(m["severity"], "dim-label")])


class Sparkline(Gtk.DrawingArea):
    """The last 24 hours of one folded row: an hourly bar each, 2 px wide and 1 px apart, in the
    row's severity colour; an hour with nothing is a 1 px stub."""

    def __init__(self, hours, severity):
        super().__init__(content_width=24 * 3 - 1, content_height=14, halign=Gtk.Align.END, valign=Gtk.Align.END)
        self.hours, self.color = hours, hex_rgb(SEVERITY_RGB.get(severity, SEVERITY_RGB["info"]))
        self.set_draw_func(self.draw)

    def draw(self, _area, cr, _width, height):
        peak = max(self.hours, default=0) or 1
        for i, n in enumerate(self.hours):
            if n:
                h = max(2, round(n / peak * height))
                cr.set_source_rgb(*self.color)
            else:
                h = 1
                cr.set_source_rgb(*EMPTY_HOUR)
            cr.rectangle(i * 3, height - h, 2, h)
            cr.fill()


class EventRow(Gtk.ListBoxRow):
    """dot | name and description over the reason | time and count over the sparkline"""

    def __init__(self, m, hours=None):
        super().__init__()
        self.event = m
        name, desc, detail = split_summary(m["source"], m["severity"], m["summary"])
        grid = Gtk.Grid(column_spacing=10)
        dot = Gtk.Box(css_classes=["sev-dot", f"sev-{m['severity']}"], valign=Gtk.Align.START, halign=Gtk.Align.START)
        grid.attach(dot, 0, 0, 1, 2)
        line = Gtk.Box(spacing=6, hexpand=True)
        # a name of up to 20 characters is never cut; a longer one gives way at 20
        long_name = len(name) > 20
        line.append(Gtk.Label(label=name, xalign=0, css_classes=["ev-name"], ellipsize=Pango.EllipsizeMode.END if long_name else Pango.EllipsizeMode.NONE,
                              width_chars=20 if long_name else -1, max_width_chars=20 if long_name else -1))
        if desc:
            line.append(Gtk.Label(label=desc, xalign=0, hexpand=True, ellipsize=Pango.EllipsizeMode.END, css_classes=["ev-desc"]))
        grid.attach(line, 1, 0, 1, 1)
        grid.attach(Gtk.Label(label=detail or LABEL.get(m["source"], m["source"]), xalign=0, hexpand=True, width_chars=1,
                              ellipsize=Pango.EllipsizeMode.END, css_classes=["ev-detail"]), 1, 1, 1, 1)
        top = Gtk.Box(spacing=6, halign=Gtk.Align.END)
        top.append(Gtk.Label(label=local(m["ts"], "%H:%M"), css_classes=["ev-time"]))
        if m.get("count", 1) > 1:
            top.append(Gtk.Label(label=f"×{m['count']}", css_classes=["ev-count"]))
        grid.attach(top, 2, 0, 1, 1)
        grid.attach(Sparkline(hours or [0] * 24, m["severity"]), 2, 1, 1, 1)
        self.set_tooltip_text(f"{readable(m['summary'])}\n{LABEL.get(m['source'], m['source'])} · {local(m['ts'], '%a %d %b, %H:%M:%S')} · {ago(m['ts'])}"
                              + (f"\nRepeated {m['count']} times since {local(m['first_ts'], '%b %d %H:%M:%S')}" if m.get("count", 1) > 1 else ""))
        self.set_child(grid)
