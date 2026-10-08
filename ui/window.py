"""The main window."""
from collections import deque
from datetime import datetime, timezone
from gtkenv import Adw, GLib, Gtk
import data
from consts import RANGES, SOURCES
from detail import DetailMixin
from overview import OverviewMixin
from processes import ProcessesMixin
from refresh import RefreshMixin
from sidebar import SidebarMixin


class Window(Adw.ApplicationWindow, SidebarMixin, OverviewMixin, ProcessesMixin, DetailMixin, RefreshMixin):
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
        self.counts = {}  # events per severity under the current filters
        self.hourly = {}  # group key -> its last 24 hours, for the sparklines
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

        self.split = Adw.NavigationSplitView(min_sidebar_width=360, max_sidebar_width=360, sidebar_width_fraction=0.3)
        self.split.set_sidebar(self.build_sidebar())
        self.split.set_content(self.build_content())
        self.toasts = Adw.ToastOverlay(child=self.split)
        self.set_content(self.toasts)

        narrow = Adw.Breakpoint.new(Adw.BreakpointCondition.parse("max-width: 720sp"))
        narrow.add_setter(self.split, "collapsed", True)
        self.add_breakpoint(narrow)

        slash = Gtk.ShortcutController(propagation_phase=Gtk.PropagationPhase.CAPTURE)
        slash.add_shortcut(Gtk.Shortcut(trigger=Gtk.ShortcutTrigger.parse_string("slash"),
                                        action=Gtk.CallbackAction.new(lambda *_: self.focus_search())))
        self.add_controller(slash)

        self.refresh(force=True)
        GLib.timeout_add_seconds(5, self.refresh)

    def build_content(self):
        view = Adw.ToolbarView(css_classes=["content-pane"])
        header = Adw.HeaderBar(css_classes=["content-header"])
        header.set_title_widget(Gtk.Box())
        self.overview_button = Gtk.Button(icon_name="go-previous-symbolic", tooltip_text="Back to the overview", visible=False)
        self.overview_button.connect("clicked", lambda *_: self.show_overview())
        header.pack_start(self.overview_button)
        self.range = Adw.ToggleGroup(css_classes=["segmented", "range-switch"], valign=Gtk.Align.CENTER)
        for label, hours in RANGES:
            self.range.add(Adw.Toggle(name=str(hours), label=label))
        self.range.set_active_name(str(self.hours))
        self.range.connect("notify::active-name", self.on_range)
        header.pack_start(self.range)
        header.pack_end(Gtk.MenuButton(label="Go to a moment…", css_classes=["moment-button"], valign=Gtk.Align.CENTER,
                                       popover=self.build_time_picker()))
        self.copy_button = Gtk.Button(icon_name="edit-copy-symbolic", tooltip_text="Copy a text report of this moment", visible=False)
        self.copy_button.connect("clicked", lambda *_: self.copy_report())
        header.pack_end(self.copy_button)
        view.add_top_bar(header)
        self.banner = Adw.Banner(revealed=False)
        view.add_top_bar(self.banner)

        self.stack = Gtk.Stack(transition_type=Gtk.StackTransitionType.CROSSFADE)
        self.detail_scroll = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER)
        self.stack.add_named(self.build_overview(), "overview")
        self.stack.add_named(self.detail_scroll, "detail")
        view.set_content(self.stack)
        self.content_page = Adw.NavigationPage(title="Overview", tag="detail", child=view)
        return self.content_page

    def on_range(self, group, _param):
        name = group.get_active_name()
        self.hours = None if name == "None" else int(name)
        self.refresh(force=True)

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
