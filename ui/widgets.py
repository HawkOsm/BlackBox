"""Small reusable widgets: stat tiles, chart cards, event rows."""
from gtkenv import Gtk, Pango
from chart import Chart, legend
from consts import ICON, LABEL, MOMENT_ICON, TONE
from fmt import ago, local, readable


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
        self.meta = Gtk.Label(xalign=0, css_classes=["caption", "dim-label"])
        text.append(self.meta)
        self.update_age()
        box.append(text)
        if m.get("count", 1) > 1:
            box.append(Gtk.Label(
                label=f"×{m['count']}", css_classes=["pill"], valign=Gtk.Align.CENTER,
                tooltip_text=f"Repeated {m['count']} times since {local(m['first_ts'], '%b %d %H:%M:%S')}",
            ))
        self.set_child(box)

    def update_age(self):
        m = self.event
        self.meta.set_label(f"{LABEL.get(m['source'], m['source'])} · {local(m['ts'], '%H:%M')} · {ago(m['ts'])}")
