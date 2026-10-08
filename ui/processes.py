"""The process list on the overview: running apps, with pause, end and kill.

/proc is read on a worker thread every two seconds while the overview is showing. Rows are kept
per app and updated in place, so the selection and scroll position survive each refresh."""
import os
import signal
import threading
import time
from gtkenv import Adw, GLib, Gtk, Pango
from consts import HEAT
from fmt import local
from procs import Poller

EVERY_MS = 2000
PENDING_S = 5  # how long "Ending…" shows before a process that ignored SIGTERM goes back to normal
# key, header, width (None: fills), ascending by default, heat name
COLUMNS = [
    ("name", "Name", None, True, None),
    ("pid", "PID", 76, False, None),
    ("cpu", "CPU", 80, False, "cpu"),
    ("mem_mb", "Memory", 96, False, "mem"),
    ("gpu", "GPU", 68, False, "gpu"),
    ("disk_mbs", "Disk", 92, False, "disk"),
    ("status", "Status", 96, True, None),
]


def memory(mb):
    return f"{mb / 1024:.1f} GB" if mb >= 1024 else f"{mb:.0f} MB"


def cell_text(key, app):
    v = app[key]
    if key == "cpu":
        return f"{v:.1f}%"
    if key == "mem_mb":
        return memory(v)
    if key == "gpu":
        return f"{v:.0f}%"
    if key == "disk_mbs":
        return f"{v:.1f} MB/s"
    return str(v)


def heat_step(heat, v):
    """0 (no tint) to 8: alpha = min(0.32, 0.04 + v/max × 0.28), quantised to the CSS steps."""
    if v <= 0:
        return 0
    return max(1, min(8, round(v / HEAT[heat][1] * 8)))


class ProcRow(Gtk.ListBoxRow):
    def __init__(self):
        super().__init__()
        self.app = None
        box = Gtk.Box(spacing=4)
        name = Gtk.Box(spacing=8, hexpand=True, css_classes=["proc-name-cell"])
        self.name = Gtk.Label(xalign=0, css_classes=["proc-name"], ellipsize=Pango.EllipsizeMode.END, max_width_chars=28)
        self.count = Gtk.Label(xalign=0, css_classes=["proc-count"])
        self.tag = Gtk.Label(xalign=0, hexpand=True, width_chars=1, css_classes=["proc-tag"], ellipsize=Pango.EllipsizeMode.END)
        for w in (self.name, self.count, self.tag):
            name.append(w)
        box.append(name)
        self.pid = Gtk.Label(xalign=1, width_request=76, css_classes=["proc-pid"])
        box.append(self.pid)
        self.cells = {}
        for key, _, width, _, heat in COLUMNS[2:6]:
            cell = Gtk.Label(xalign=1, width_request=width, css_classes=["num"])
            self.cells[key] = (cell, heat)
            box.append(cell)
        self.status = Gtk.Label(xalign=0, width_request=96, css_classes=["status"])
        box.append(self.status)
        self.set_child(box)

    def update(self, app):
        self.app = app
        self.name.set_label(app["name"])
        n = len(app["pids"])
        self.count.set_label(f"{n} processes" if n > 1 else "")
        self.count.set_visible(n > 1)
        self.tag.set_label(app["tag"] or "")
        self.tag.set_tooltip_text(app["tag"])
        self.pid.set_label(str(app["pid"]))
        for key, (cell, heat) in self.cells.items():
            cell.set_label(cell_text(key, app))
            cell.set_css_classes(["num"] + ([f"heat-{heat}-{s}"] if (s := heat_step(heat, app[key])) else []))
        self.status.set_label(app["status"])
        tone = {"Stopped": "stopped", "Ending…": "ending", "Killing…": "ending"}.get(app["status"])
        self.status.set_css_classes(["status"] + ([tone] if tone else []))


class ProcessesMixin:
    def build_processes(self):
        self.poller = Poller()
        self.proc_rows = {}  # pid of the app's top process -> ProcRow
        self.proc_pending = {}  # pid -> (status shown, until)
        self.proc_busy = False
        self.sel_pid = None
        self.sort_key, self.sort_asc = "cpu", False
        self.crash_tags = {}

        card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, vexpand=True, css_classes=["card"], overflow=Gtk.Overflow.HIDDEN)
        head = Gtk.Box(spacing=10, css_classes=["proc-head"])
        head.append(Gtk.Label(label="Processes", css_classes=["card-title"]))
        self.proc_summary = Gtk.Label(xalign=0, hexpand=True, css_classes=["meta"], ellipsize=Pango.EllipsizeMode.END)
        head.append(self.proc_summary)
        self.pause_button = Gtk.Button(label="Pause", css_classes=["act"], tooltip_text="Pause or resume every process of the app (SIGSTOP / SIGCONT)")
        self.pause_button.connect("clicked", lambda *_: self.toggle_pause())
        end = Gtk.Button(label="End task", css_classes=["act"], tooltip_text="Ask the app to quit (SIGTERM)")
        end.connect("clicked", lambda *_: self.end_app(signal.SIGTERM))
        kill = Gtk.Button(label="Force kill", css_classes=["act", "danger"], tooltip_text="Kill every process of the app at once (SIGKILL)")
        kill.connect("clicked", lambda *_: self.end_app(signal.SIGKILL))
        self.proc_actions = [self.pause_button, end, kill]
        buttons = Gtk.Box(spacing=6)
        for b in self.proc_actions:
            buttons.append(b)
        head.append(buttons)
        card.append(head)

        self.proc_header = Gtk.Box(spacing=4, css_classes=["proc-columns"])
        self.header_buttons = {}
        for key, title, width, asc, _ in COLUMNS:
            label = Gtk.Label(xalign=1 if width and key != "status" else 0, hexpand=True)
            button = Gtk.Button(child=label, hexpand=width is None, width_request=width or -1)
            button.connect("clicked", self.on_sort, key, asc)
            self.header_buttons[key] = (button, label, title)
            self.proc_header.append(button)
        card.append(self.proc_header)

        self.proc_list = Gtk.ListBox(selection_mode=Gtk.SelectionMode.SINGLE, css_classes=["proc-list"])
        self.proc_list.set_sort_func(self.proc_order)
        self.proc_list.connect("row-selected", self.on_proc_selected)
        card.append(Gtk.ScrolledWindow(child=self.proc_list, vexpand=True, hscrollbar_policy=Gtk.PolicyType.NEVER))
        self.update_proc_header([])
        self.update_proc_summary()
        GLib.timeout_add(EVERY_MS, self.poll_processes)
        self.poll_processes()
        return card

    # ---- reading /proc

    def poll_processes(self):
        """Also the timer callback, hence the True. Skipped while the overview is out of sight."""
        if not self.proc_busy and self.current is None and self.get_mapped():
            self.proc_busy = True
            threading.Thread(target=self.read_processes, daemon=True).start()
        return True

    def read_processes(self):
        try:
            apps = self.poller.poll(time.monotonic())
        except OSError:
            apps = None
        GLib.idle_add(self.apply_processes, apps)

    def apply_processes(self, apps):
        self.proc_busy = False
        if apps is None:
            return GLib.SOURCE_REMOVE
        now = time.monotonic()
        me = os.getpid()
        self.proc_pending = {pid: p for pid, p in self.proc_pending.items() if p[1] > now}
        seen = set()
        for app in apps:
            pid = app["pid"]
            seen.add(pid)
            app["status"] = self.proc_pending[pid][0] if pid in self.proc_pending else "Stopped" if app["stopped"] else "Running"
            if app["stopped"]:
                app["cpu"] = app["gpu"] = app["disk_mbs"] = 0.0
            app["tag"] = self.proc_tag(app, me)
            row = self.proc_rows.get(pid)
            if row is None:
                row = self.proc_rows[pid] = ProcRow()
                self.proc_list.append(row)
            row.update(app)
        for pid in [p for p in self.proc_rows if p not in seen]:
            self.proc_list.remove(self.proc_rows.pop(pid))
            if pid == self.sel_pid:
                self.sel_pid = None
        self.proc_list.invalidate_sort()
        self.update_proc_header(apps)
        self.update_proc_summary()
        return GLib.SOURCE_REMOVE

    def proc_tag(self, app, me):
        if me in app["pids"]:
            return "this window"
        if app["name"] == "blackbox" and app["exe"] and app["exe"].endswith("/blackbox"):
            return "this app's recorder"
        crashes = self.crash_tags.get(app["name"])
        if crashes:
            n, first = crashes
            return f"crashed {n}× since {local(first, '%d %b')}" if n > 1 else f"crashed on {local(first, '%d %b')}"
        return None

    # ---- sorting and selection

    def proc_order(self, a, b):
        x, y = a.app, b.app
        if x is None or y is None:
            return 0
        key = self.sort_key
        vx, vy = (x[key].lower(), y[key].lower()) if key in ("name", "status") else (x[key], y[key])
        if vx == vy:
            vx, vy = x["pid"], y["pid"]  # a stable order between equals, so rows don't shuffle
            return -1 if vx < vy else 1
        less = vx < vy
        return (-1 if less else 1) if self.sort_asc else (1 if less else -1)

    def on_sort(self, _button, key, asc):
        if key == self.sort_key:
            self.sort_asc = not self.sort_asc
        else:
            self.sort_key, self.sort_asc = key, asc
        self.proc_list.invalidate_sort()
        self.update_proc_header([row.app for row in self.proc_rows.values() if row.app])

    def update_proc_header(self, apps):
        totals = {"cpu": f" {sum(a['cpu'] for a in apps):.0f}%", "mem_mb": f" {sum(a['mem_mb'] for a in apps) / 1024:.1f} GB"}
        for key, (button, label, title) in self.header_buttons.items():
            arrow = (" ↑" if self.sort_asc else " ↓") if key == self.sort_key else ""
            label.set_label(f"{title}{totals.get(key, '')}{arrow}")
            (button.add_css_class if key == self.sort_key else button.remove_css_class)("sorted")

    def on_proc_selected(self, _list, row):
        self.sel_pid = row.app["pid"] if row is not None and row.app else None
        self.update_proc_summary()

    def selected_app(self):
        row = self.proc_rows.get(self.sel_pid)
        return row.app if row is not None else None

    def update_proc_summary(self):
        app = self.selected_app()
        if app is None:
            self.proc_summary.set_label(f"{len(self.proc_rows)} apps · select one to act on it")
        else:
            self.proc_summary.set_label(f"{app['name']} · pid {app['pid']} selected")
        for b in self.proc_actions:
            b.set_sensitive(app is not None)
        self.pause_button.set_label("Resume" if app is not None and app["stopped"] else "Pause")

    # ---- signals

    def signal_app(self, app, sig, pids):
        """Sends `sig` to `pids`; a toast and False when not one of them could be signalled."""
        sent, denied = 0, False
        for pid in pids:
            try:
                os.kill(pid, sig)
                sent += 1
            except ProcessLookupError:
                pass
            except PermissionError:
                denied = True
        if not sent:
            why = "it belongs to another user" if denied else "it has already exited"
            self.toasts.add_toast(Adw.Toast(title=f"Could not signal {app['name']}: {why}", timeout=3))
        return sent > 0

    def toggle_pause(self):
        app = self.selected_app()
        if app is None:
            return
        resume = app["stopped"]
        sig = signal.SIGCONT if resume else signal.SIGSTOP
        if self.signal_app(app, sig, app["pids"]):
            app["stopped"] = not resume
            app["status"] = "Running" if resume else "Stopped"
            if not resume:
                app["cpu"] = app["gpu"] = app["disk_mbs"] = 0.0
            self.proc_rows[app["pid"]].update(app)
            self.update_proc_summary()
            verb = "Resumed" if resume else "Paused"
            self.toasts.add_toast(Adw.Toast(title=f"{verb} {app['name']} ({'SIGCONT' if resume else 'SIGSTOP'})", timeout=3))

    def end_app(self, sig):
        app = self.selected_app()
        if app is None:
            return
        # SIGTERM goes to the top process, which shuts its own helpers down; SIGKILL leaves no
        # one to do that, so it goes to every process of the app
        pids = [app["pid"]] if sig == signal.SIGTERM else app["pids"]
        if app["stopped"] and sig == signal.SIGTERM:
            self.signal_app(app, signal.SIGCONT, app["pids"])  # a stopped process cannot act on SIGTERM
        if not self.signal_app(app, sig, pids):
            return
        status = "Ending…" if sig == signal.SIGTERM else "Killing…"
        self.proc_pending[app["pid"]] = (status, time.monotonic() + PENDING_S)
        app["status"] = status
        self.proc_rows[app["pid"]].update(app)
        if sig == signal.SIGTERM:
            title = f"Asked {app['name']} to quit (SIGTERM) · pid {app['pid']}"
        else:
            title = f"Sent SIGKILL to {app['name']} · pid {app['pid']}"
        self.toasts.add_toast(Adw.Toast(title=title, timeout=3))
        GLib.timeout_add(700, lambda: self.poll_processes() and False)
