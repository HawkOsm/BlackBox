"""The page for one event or moment."""
import sqlite3
from gtkenv import Adw, Gdk, GLib, Gtk, Pango
import data
from consts import CHARTS, FIELDS, LABEL, WINDOWS
from fmt import ago, field_value, local, offset, readable
from widgets import event_icon, rows_table, spec_card


class DetailMixin:
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
        ticks = [(data.epoch(e["ts"]), e["summary"]) for e in ctx["messages"] if (e["source"], e["ref_id"]) != self.selected]
        rows_for = {"usage": ctx["sysstat"], "temperature": ctx["sysstat"], "power": ctx["power"]}
        for spec in CHARTS:
            title = f"{spec['title']} around this moment"
            box.append(spec_card(spec, title, rows_for[spec["name"]], t - self.window_secs, t + self.window_secs,
                                 ticks, marker=t, height=220 if spec["name"] == "usage" else 180,
                                 extra=windows if spec["name"] == "usage" else None))

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
                if key.endswith("_id") or key in ("ts", "changes"):
                    continue
                row = Adw.ActionRow(title=FIELDS.get(key, key.replace("_", " ").capitalize()), subtitle=field_value(key, value),
                                    use_markup=False, css_classes=["property"])
                row.set_subtitle_selectable(True)
                group.add(row)
            box.append(group)

        if detail.get("changes"):
            lines = detail["changes"].splitlines()
            group = Adw.PreferencesGroup(title=f"Packages ({len(lines)})")
            expander = Gtk.Expander(label="Every package in this transaction", expanded=len(lines) <= 12)
            expander.set_child(Gtk.Label(label=detail["changes"], xalign=0, wrap=True, wrap_mode=Pango.WrapMode.CHAR,
                                         selectable=True, css_classes=["mono"], margin_top=6))
            card = Gtk.Box(css_classes=["card", "chart-card"])
            card.append(expander)
            group.add(card)
            box.append(group)

        if m.get("source") != "packages":
            changes, total = self.db.changes_before(m["ts"])
            if changes:
                more = f", newest {len(changes)} shown" if total > len(changes) else ""
                group = Adw.PreferencesGroup(title="Changed in the week before",
                                             description=f"{total} package transactions{more}")
                for c in changes:
                    row = Adw.ActionRow(title=c["summary"], subtitle=f"{offset(data.epoch(c['ts']) - t)} · {local(c['ts'], '%a %d %b, %H:%M')}",
                                        use_markup=False, activatable=True, title_lines=2)
                    row.add_suffix(Gtk.Image(icon_name="go-next-symbolic", css_classes=["dim-label"]))
                    row.connect("activated", lambda _r, e=c: self.show_detail(e))
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
        rows_for = {"usage": ctx["sysstat"], "temperature": ctx["sysstat"], "power": ctx["power"]}
        for spec in CHARTS:
            if any(r.get(key) is not None for r in rows_for[spec["name"]] for key in spec["series"]):
                out += ["", f"{spec['title']}, ±{self.window_secs // 60} min"]
                out += ["  " + line for line in rows_table(spec, rows_for[spec["name"]]).splitlines()]
        out += ["", "Events around it"]
        out += [f"  {offset(data.epoch(e['ts']) - t):>8}  {e['severity']:<7} {LABEL.get(e['source'], e['source']):<8} {e['summary']}" for e in ctx["messages"]]
        Gdk.Display.get_default().get_clipboard().set("\n".join(out) + "\n")
        self.toasts.add_toast(Adw.Toast(title="Report copied to the clipboard", timeout=2))
