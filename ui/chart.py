"""The over-time line chart drawn with cairo: one axis in a single unit (%, °C or W)."""
import bisect
from gtkenv import Adw, GLib, Gtk, Pango, PangoCairo
import data
from consts import SERIES
from fmt import local_epoch


def hex_rgb(h):
    return tuple(int(h[i:i + 2], 16) / 255 for i in (1, 3, 5))


def dark_mode():
    return Adw.StyleManager.get_default().get_dark()


NICE_MAX = (5, 10, 20, 30, 40, 50, 60, 80, 100, 120, 150, 200, 300, 400, 500)  # all halve evenly


class Chart(Gtk.DrawingArea):
    """Lines on one axis, with event ticks, a marked moment, direct labels at the line ends and
    a hover crosshair with a tooltip. `ymax` fixes the top of the axis (100 for percent); None
    fits it to the data, from zero, up to the next round number."""

    PAD_L, PAD_R, PAD_T, PAD_B = 50, 124, 10, 26

    def __init__(self, height=210, unit="%", ymax=100):
        super().__init__(content_height=height, hexpand=True)
        self.unit, self.ymax = unit, ymax
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

    def top(self):
        if self.ymax:
            return self.ymax
        peak = max((v for _, pts in self.series for _, v in pts), default=1) * 1.1
        return next((n for n in NICE_MAX if n >= peak), -(-peak // 100) * 100)

    def value(self, v, top=None):
        """A reading with its unit; watts get a decimal on a small axis, where 7.5 and 8 differ."""
        if self.unit == "%":
            return f"{v:.0f}%"
        if self.unit == "W" and (top or self.top()) <= 30:
            return f"{v:.1f} W"
        return f"{v:.0f} {self.unit}"

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

        top = self.top()

        def Y(v):
            return T + (1 - max(0.0, min(float(top), v)) / top) * ph

        cr.set_line_width(1)
        for v in (0, top / 2, top):
            y = round(Y(v)) + 0.5
            cr.set_source_rgba(*ink(0.14 if v == 0 else 0.07))
            cr.move_to(L, y)
            cr.line_to(L + pw, y)
            cr.stroke()
            self.label(cr, self.value(v, top).replace(".0 ", " "), L - 8, y, ink(0.55), "right")
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
            self.label(cr, f"{SERIES[key][0]} {self.value(v, top)}", min(xend + 8, L + pw + 8), y, ink(0.8))

        if self.hover is not None and L <= self.hover <= L + pw:
            self.draw_hover(cr, width, ink, dark, X, Y, L, T, pw, ph, top)

    def draw_hover(self, cr, width, ink, dark, X, Y, L, T, pw, ph, top):
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
            found = nearest(pts, t, self.gap_limit(pts))
            if found is None:
                continue
            pt, pv = found
            color = hex_rgb(SERIES[key][2 if dark else 1])
            cr.set_source_rgb(*surface)
            cr.arc(X(pt), Y(pv), 6, 0, 6.2832)
            cr.fill()
            cr.set_source_rgb(*color)
            cr.arc(X(pt), Y(pv), 4, 0, 6.2832)
            cr.fill()
            lines.append((color, f"{SERIES[key][0]}  {self.value(pv, top)}"))
        near_ticks = [text for tt, text in self.ticks if abs(X(tt) - hx) <= 5]
        for text in near_ticks[:3]:
            lines.append(("event", text if len(text) <= 64 else text[:63] + "…"))

        tooltip(self, cr, lines, hx, T + 4, width, dark)


def tooltip(widget, cr, lines, hx, by, width, dark):
    """The hover box beside the crosshair: a bold first line, then lines with a colour swatch
    (an (r, g, b) tuple) or an event mark ("event")."""
    layouts = []
    for color, text in lines:
        layout = widget.create_pango_layout(None)
        weight = ' weight="bold"' if color is None else ""
        layout.set_markup(f'<span size="small"{weight}>{GLib.markup_escape_text(text)}</span>', -1)
        layouts.append((color, layout, *layout.get_pixel_size()))
    box_w = max(w for _, _, w, _ in layouts) + 34
    box_h = sum(h for *_, h in layouts) + 4 * (len(layouts) - 1) + 16
    bx = hx + 14 if hx + 14 + box_w < width else hx - 14 - box_w
    bg, text_rgba = ((0.96, 0.96, 0.97, 0.97), (0.1, 0.1, 0.12, 1)) if dark else ((0.14, 0.14, 0.16, 0.95), (1, 1, 1, 1))
    cr.set_source_rgba(*bg)
    rounded(cr, bx, by, box_w, box_h, 8)
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


def rounded(cr, x, y, w, h, r):
    cr.new_sub_path()
    cr.arc(x + w - r, y + r, r, -1.5708, 0)
    cr.arc(x + w - r, y + h - r, r, 0, 1.5708)
    cr.arc(x + r, y + h - r, r, 1.5708, 3.1416)
    cr.arc(x + r, y + r, r, 3.1416, 4.7124)
    cr.close_path()


def nearest(pts, t, limit):
    """The sample in `pts` closest to `t`, or None when none is within `limit` seconds."""
    times = [p[0] for p in pts]
    i = bisect.bisect_left(times, t)
    near = min((j for j in (i - 1, i) if 0 <= j < len(pts)), key=lambda j: abs(times[j] - t), default=None)
    if near is None or abs(times[near] - t) > limit:
        return None
    return pts[near]


ERROR, WARNING, RULE, META = hex_rgb("#EF4444"), hex_rgb("#F2B33D"), hex_rgb("#1C1C21"), hex_rgb("#62646C")
# (label, unit, series, fixed top or None to fit, height in px)
LANES = [
    ("Usage %", "%", ("cpu_pct", "mem_pct", "gpu_pct"), 100, 72),
    ("Temp °C", "°C", ("cpu_temp", "gpu_temp", "nvme_temp"), None, 46),
    ("Power W", "W", ("cpu_watt", "platform_watt"), None, 46),
]


class GlanceChart(Gtk.DrawingArea):
    """The overview's one chart: event density, usage, temperature and power as lanes stacked
    on a shared time axis, with hatched gaps where nothing was sampled, a dashed line at every
    error-level boot or crash, and one crosshair across all lanes."""

    LABEL_W, EVENTS_H, GAP, AXIS_H = 80, 30, 8, 18

    def __init__(self):
        height = self.EVENTS_H + sum(lane[4] for lane in LANES) + self.GAP * len(LANES) + self.AXIS_H
        super().__init__(content_height=height, hexpand=True)
        self.t0 = self.t1 = 0
        self.bars = []
        self.lanes = []  # [(lane, [(key, [(t, v)])], lo, hi)]
        self.markers = []
        self.samples = []  # sample times, for the gaps
        self.limit = 1e12
        self.hover = None
        self.set_draw_func(self.draw)
        motion = Gtk.EventControllerMotion()
        motion.connect("motion", lambda _c, x, _y: self.set_hover(x))
        motion.connect("leave", lambda _c: self.set_hover(None))
        self.add_controller(motion)

    def set_hover(self, x):
        self.hover = x
        self.queue_draw()

    def set_data(self, g):
        self.t0, self.t1, self.bars = g["t0"], g["t1"], g["bars"]
        self.markers = sorted((data.epoch(m["ts"]), m["summary"]) for m in g["markers"])
        rows = {"cpu_watt": g["power"], "platform_watt": g["power"]}
        self.samples = sorted({data.epoch(r["ts"]) for r in g["load"]} | {data.epoch(r["ts"]) for r in g["power"]})
        self.limit = max(3 * g["bucket_s"], 60)
        self.lanes = []
        for lane in LANES:
            series = []
            for key in lane[2]:
                pts = [(data.epoch(r["ts"]), r[key]) for r in rows.get(key, g["load"]) if r.get(key) is not None]
                if pts:
                    series.append((key, pts))
            values = [v for _, pts in series for _, v in pts]
            if lane[3]:
                lo, hi = 0, lane[3]
            elif values:  # a small lane: fit it to the data with a little headroom
                lo, hi = max(0, min(values) - 5), max(values) + 5
            else:
                lo, hi = 0, 1
            self.lanes.append((lane, series, lo, hi))
        self.queue_draw()

    def gaps(self):
        """Stretches of the range with no sample within `limit`: the machine was off or asleep."""
        out, prev = [], self.t0 - self.limit / 2
        for t in self.samples + [self.t1 + self.limit / 2]:
            if t - prev > self.limit:
                out.append((max(self.t0, prev + self.limit / 2), min(self.t1, t - self.limit / 2)))
            prev = t
        return [(a, b) for a, b in out if b > a]

    def label(self, cr, text, x, y, rgb, align="left", px=11):
        layout = self.create_pango_layout(None)
        font = self.get_pango_context().get_font_description().copy()
        font.set_absolute_size(px * Pango.SCALE)
        layout.set_font_description(font)
        layout.set_markup(f'<span font_features="tnum">{GLib.markup_escape_text(text)}</span>', -1)
        w, h = layout.get_pixel_size()
        x -= {"left": 0, "center": w / 2, "right": w}[align]
        cr.set_source_rgb(*rgb)
        cr.move_to(x, y - h / 2)
        PangoCairo.show_layout(cr, layout)

    def draw(self, _area, cr, width, height):
        L, pw = self.LABEL_W, width - self.LABEL_W
        span = self.t1 - self.t0
        if pw < 60 or span <= 0:
            return

        def X(t):
            return L + (t - self.t0) / span * pw

        # lane boxes: (top, height)
        boxes, y = [], 0
        boxes.append((y, self.EVENTS_H))
        y += self.EVENTS_H + self.GAP
        for lane in LANES:
            boxes.append((y, lane[4]))
            y += lane[4] + self.GAP
        plot_bottom = y - self.GAP

        # gaps: 135° hatching through every lane
        cr.save()
        for a, b in self.gaps():
            cr.rectangle(X(a), 0, X(b) - X(a), plot_bottom)
        cr.clip()
        cr.set_source_rgba(1, 1, 1, 0.035)
        cr.set_line_width(4)  # 4 px stripes 4 px apart, measured across the 45° stripe: 8·√2 along x
        for x in range(int(L) - int(plot_bottom), int(width) + 12, 11):
            cr.move_to(x, plot_bottom)
            cr.line_to(x + plot_bottom, 0)
        cr.stroke()
        cr.restore()

        # events lane: errors stacked on warnings
        top, h = boxes[0]
        self.label(cr, "Events", 0, top + h / 2, META, px=11.5)
        peak = max((b["error"] + b["warning"] for b in self.bars), default=0)
        if self.bars and peak:
            slot = pw / len(self.bars)
            for i, b in enumerate(self.bars):
                x, w = L + i * slot + 1, max(1, slot - 2)
                wh = b["warning"] / peak * h
                eh = b["error"] / peak * h
                cr.set_source_rgba(*WARNING, 0.55)
                cr.rectangle(x, top + h - wh, w, wh)
                cr.fill()
                cr.set_source_rgb(*ERROR)
                cr.rectangle(x, top + h - wh - eh, w, eh)
                cr.fill()

        # line lanes
        cr.set_line_join(1)
        cr.set_line_cap(1)
        for (lane, series, lo, hi), (top, h) in zip(self.lanes, boxes[1:]):
            self.label(cr, lane[0], 0, top + h / 2, META, px=11.5)
            cr.set_source_rgb(*RULE)
            cr.set_line_width(1)
            cr.move_to(L, top + h + 0.5)
            cr.line_to(width, top + h + 0.5)
            cr.stroke()
            cr.set_line_width(1.5)
            for key, pts in series:
                cr.set_source_rgb(*hex_rgb(SERIES[key][2]))
                prev = None
                for t, v in pts:
                    py = top + 2 + (1 - (min(hi, max(lo, v)) - lo) / (hi - lo)) * (h - 4)
                    if prev is None or t - prev > self.limit:
                        cr.move_to(X(t), py)
                        cr.line_to(X(t) + 0.01, py)
                    else:
                        cr.line_to(X(t), py)
                    prev = t
                cr.stroke()

        # error markers through every lane
        cr.set_source_rgb(*ERROR)
        cr.set_line_width(1)
        cr.set_dash([3, 3])
        for t, _ in self.markers:
            if self.t0 <= t <= self.t1:
                x = round(X(t)) + 0.5
                cr.move_to(x, 0)
                cr.line_to(x, plot_bottom)
        cr.stroke()
        cr.set_dash([])

        # axis: 7 labels, the last one "now"
        fmt = "%H:%M" if span <= 2 * 86400 else "%d %b"
        for i in range(7):
            t = self.t0 + i / 6 * span
            text = "now" if i == 6 else local_epoch(t, fmt)
            self.label(cr, text, L + i / 6 * pw, plot_bottom + self.AXIS_H / 2 + 3, META, ("left", "center", "center", "center", "center", "center", "right")[i])

        if self.hover is not None and L <= self.hover <= width:
            self.draw_hover(cr, width, X, boxes, plot_bottom)

    def draw_hover(self, cr, width, X, boxes, plot_bottom):
        hx = self.hover
        t = self.t0 + (hx - self.LABEL_W) / (width - self.LABEL_W) * (self.t1 - self.t0)
        cr.set_line_width(1)
        cr.set_source_rgba(1, 1, 1, 0.3)
        cr.move_to(round(hx) + 0.5, 0)
        cr.line_to(round(hx) + 0.5, plot_bottom)
        cr.stroke()
        span = self.t1 - self.t0
        lines = [(None, local_epoch(t, "%a %H:%M" if span > 86400 else "%H:%M"))]
        if self.bars:
            b = self.bars[min(len(self.bars) - 1, int((t - self.t0) / span * len(self.bars)))]
            if b["error"] or b["warning"]:
                lines.append(("event", f"{b['error']} errors, {b['warning']} warnings"))
        for (lane, series, lo, hi), (top, h) in zip(self.lanes, boxes[1:]):
            for key, pts in series:
                found = nearest(pts, t, self.limit)
                if found is None:
                    continue
                pt, pv = found
                color = hex_rgb(SERIES[key][2])
                py = top + 2 + (1 - (min(hi, max(lo, pv)) - lo) / (hi - lo)) * (h - 4)
                cr.set_source_rgb(0.055, 0.055, 0.063)
                cr.arc(X(pt), py, 4.5, 0, 6.2832)
                cr.fill()
                cr.set_source_rgb(*color)
                cr.arc(X(pt), py, 3, 0, 6.2832)
                cr.fill()
                value = f"{pv:.0f}%" if lane[1] == "%" else f"{pv:.0f} °C" if lane[1] == "°C" else f"{pv:.1f} W"
                lines.append((color, f"{SERIES[key][0]}  {value}"))
        for mt, text in self.markers:
            if abs(X(mt) - hx) <= 5:
                lines.append(("event", text if len(text) <= 64 else text[:63] + "…"))
        tooltip(self, cr, lines[:12], hx, 4, width, True)


def legend(keys):
    box = Gtk.Box(spacing=12, valign=Gtk.Align.CENTER)
    for key in keys:
        item = Gtk.Box(spacing=6)
        item.append(Gtk.Box(css_classes=["swatch", f"swatch-{key}"], valign=Gtk.Align.CENTER))
        item.append(Gtk.Label(label=SERIES[key][0], css_classes=["caption"]))
        box.append(item)
    return box
