"""The over-time line chart drawn with cairo: one axis in a single unit (%, °C or W)."""
import bisect
from gtkenv import Adw, GLib, Gtk, PangoCairo
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
            lines.append((color, f"{SERIES[key][0]}  {self.value(pv, top)}"))
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
