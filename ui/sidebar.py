"""The event list and its filters."""
from gtkenv import Adw, Gtk
from consts import SCOPES, SOURCES


class SidebarMixin:
    def build_sidebar(self):
        view = Adw.ToolbarView(css_classes=["sidebar-pane"])
        header = Adw.HeaderBar()
        row = Gtk.Box(spacing=8, hexpand=True)
        field = Gtk.Box(spacing=8, hexpand=True, css_classes=["search-field"])
        self.search = Gtk.SearchEntry(placeholder_text="Search events", hexpand=True, valign=Gtk.Align.CENTER)
        self.search.set_search_delay(300)  # one read per pause in typing, not per keystroke
        self.search.set_key_capture_widget(self)  # typing anywhere searches
        self.search.connect("search-changed", lambda *_: self.refresh(force=True))
        field.append(self.search)
        field.append(Gtk.Label(label="/", css_classes=["key-hint"], valign=Gtk.Align.CENTER))
        row.append(field)
        row.append(Gtk.MenuButton(icon_name="open-menu-symbolic", tooltip_text="Sources", css_classes=["filter-button"],
                                  valign=Gtk.Align.CENTER, popover=self.build_filters()))
        header.set_title_widget(row)
        view.add_top_bar(header)

        self.scope = Adw.ToggleGroup(homogeneous=True, css_classes=["segmented", "scope-switch"])
        for name, label, _ in SCOPES:
            self.scope.add(Adw.Toggle(name=name, label=label))
        self.scope.set_active_name("problems")
        self.scope.connect("notify::active-name", lambda *_: self.refresh(force=True))
        view.add_top_bar(self.scope)

        self.list = Gtk.ListBox(selection_mode=Gtk.SelectionMode.SINGLE, css_classes=["event-list"])
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

        footer = Gtk.Box(css_classes=["sidebar-footer"])
        self.footer = Gtk.Label(xalign=0, hexpand=True, css_classes=["tnum"])
        self.footer_cap = Gtk.Label(xalign=1, css_classes=["tnum"])
        footer.append(self.footer)
        footer.append(self.footer_cap)
        view.add_bottom_bar(footer)
        return Adw.NavigationPage(title="Blackbox", tag="events", child=view)

    def build_filters(self):
        group = Gtk.ListBox(css_classes=["boxed-list"], selection_mode=Gtk.SelectionMode.NONE)
        for key, label in SOURCES:
            row = Adw.SwitchRow(title=label, active=True)
            row.connect("notify::active", self.on_source, key)
            group.append(row)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10, width_request=280,
                      margin_top=6, margin_bottom=6, margin_start=6, margin_end=6)
        box.append(Gtk.Label(label="Sources", xalign=0, css_classes=["heading"]))
        box.append(group)
        return Gtk.Popover(child=box)

    def set_scope_counts(self, counts):
        """`Problems {n}` and `Errors {n}` on the scope switch."""
        n = {"problems": counts.get("error", 0) + counts.get("warning", 0), "errors": counts.get("error", 0)}
        for name, label, _ in SCOPES:
            toggle = self.scope.get_toggle_by_name(name)
            toggle.set_label(f"{label} {n[name]:,}" if name in n else label)

    def focus_search(self, *_):
        if self.search.has_focus():
            return False  # "/" typed into the search itself
        self.search.grab_focus()
        return True

    def on_source(self, row, _param, key):
        (self.sources.add if row.get_active() else self.sources.discard)(key)
        self.refresh(force=True)
