"""The event list and its filters."""
from gtkenv import Adw, GObject, Gtk, Pango
from consts import RANGES, SCOPES, SOURCES
from fmt import day_label


class SidebarMixin:
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
        self.search.set_search_delay(300)  # one read per pause in typing, not per keystroke
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
        self.list_scroll = Gtk.ScrolledWindow(child=self.list, vexpand=True, hscrollbar_policy=Gtk.PolicyType.NEVER)
        adjustment = self.list_scroll.get_vadjustment()
        adjustment.connect("value-changed", self.on_list_scroll)
        adjustment.connect("changed", self.on_list_scroll)  # content grew: the next page may be due
        view.set_content(self.list_scroll)

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
