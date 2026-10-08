"""The main window."""
from collections import deque
from datetime import datetime, timezone
from gtkenv import Adw, GLib, Gtk
import data
from consts import MOMENT_ICON, SOURCES
from detail import DetailMixin
from overview import OverviewMixin
from refresh import RefreshMixin
from sidebar import SidebarMixin


class Window(Adw.ApplicationWindow, SidebarMixin, OverviewMixin, DetailMixin, RefreshMixin):
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
        self.last_overview = None
        self.pending = deque()  # events fetched but not yet turned into rows
        self.fill_active = False
        self.cursor = None  # where the next older page starts; None when there is no more
        self.pages_loaded = 0
        self.loaded_rows = 0
        self.loaded_events = 0
        self.page_query = None
        self.last_summary = None
        self.refresh_busy = False
        self.refresh_force = False

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
