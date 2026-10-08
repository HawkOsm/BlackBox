"""Keeping the event list and overview up to date.

The database is read on a worker thread and the window only applies finished results, so a
slow query on a huge database can no longer freeze the window. One read runs at a time; a
filter change made meanwhile restarts it, and the timer's own ticks are simply skipped.

The list is paged: a change of filter or range reads only the newest page, and older pages
are read as the list is scrolled towards its end, up to MAX_ROWS rows."""
import sqlite3
import threading
import time
from datetime import datetime, timedelta, timezone
from gtkenv import Adw, GLib
import data
from consts import MAX_ROWS, PAGE_ROWS, SCOPES
from widgets import EventRow

FILL_CHUNK = 30  # rows built per idle callback
OVERVIEW_EVERY = 30  # seconds between overview re-reads
NEAR_END = 600  # px from the end of the list at which the next page is read


class RefreshMixin:
    def since(self):
        if self.hours is None:
            return None
        return (datetime.now(timezone.utc) - timedelta(hours=self.hours)).strftime(data.FMT)

    def refresh(self, force=False):
        """Also the 5-second timer callback, hence the True."""
        self.refresh_force = self.refresh_force or force
        if not self.refresh_busy:
            self.start_refresh()
        return True

    def start_refresh(self):
        force, self.refresh_force = self.refresh_force, False
        self.refresh_busy = True
        # Once older pages are loaded the timer leaves the list alone: replacing it would throw
        # away what was scrolled to. Only a change of filter (force) starts over from page one.
        want_rows = force or self.pages_loaded <= 1
        if want_rows:
            severities = next(sev for name, _, sev in SCOPES if name == self.scope.get_active_name())
            self.page_query = (severities, set(self.sources), self.search.get_text(), self.since())
        overview = self.current is None and (force or time.monotonic() - self.overview_at > OVERVIEW_EVERY)
        threading.Thread(target=self.fetch, args=(self.page_query, want_rows, overview, force), daemon=True).start()

    def fetch(self, query, want_rows, overview, force):
        """Worker thread: touches only the database, never a widget."""
        try:
            result = {"summary": self.db.summary()}
            if want_rows:
                result["page"] = self.db.messages_page(*query, size=PAGE_ROWS)
            if overview:
                result["overview"] = self.db.overview_page(query[1])
        except (sqlite3.Error, OSError):
            result = None
        GLib.idle_add(self.apply_refresh, result, force)

    def apply_refresh(self, result, force):
        self.refresh_busy = False
        if self.refresh_force:
            # the filters changed while this was reading: its rows are already out of date
            self.start_refresh()
            return GLib.SOURCE_REMOVE
        if result is None:
            self.title.set_subtitle("Cannot read the database")
            self.banner.set_title("Cannot read the database. Is the collector installed and running?")
            self.banner.set_revealed(True)
            return GLib.SOURCE_REMOVE
        summary = self.last_summary = result["summary"]
        age = summary["sample_age_s"]
        if age is not None and age < 45:
            self.title.set_subtitle("Recording")
            self.banner.set_revealed(False)
        elif age is None:
            self.title.set_subtitle("No data yet")
            self.banner.set_revealed(False)
        else:
            self.title.set_subtitle("Collector stopped?")
            self.banner.set_title(f"Nothing recorded for {max(age // 60, 1)} min. Is the collector running?")
            self.banner.set_revealed(True)

        page = result.get("page")
        if page is not None:
            # rebuilt only when the rows change; "3 min ago" is refreshed in place
            key = tuple((r["source"], r["ref_id"], r["summary"], r["count"]) for r in page["rows"])
            if force or key != self.shown_key:
                self.shown_key = key
                self.cursor = page["cursor"]
                self.pages_loaded = 1
                self.loaded_events = sum(r["count"] for r in page["rows"])
                self.loaded_rows = len(page["rows"])
                self.fill_list(page["rows"], replace=True)
                if force:
                    self.list_scroll.get_vadjustment().set_value(0)
            else:
                self.update_ages()
        self.update_footer()
        if "overview" in result and self.current is None:
            self.render_overview(result["overview"], summary)
        return GLib.SOURCE_REMOVE

    # ---- older pages, read while scrolling

    def on_list_scroll(self, adjustment):
        if adjustment.get_upper() - (adjustment.get_value() + adjustment.get_page_size()) < NEAR_END:
            self.load_more()

    def load_more(self):
        if self.refresh_busy or self.pending or self.cursor is None or self.loaded_rows >= MAX_ROWS:
            return
        self.refresh_busy = True
        threading.Thread(target=self.fetch_more, args=(self.page_query, self.cursor), daemon=True).start()

    def fetch_more(self, query, cursor):
        try:
            page = self.db.messages_page(*query, before=cursor, size=PAGE_ROWS)
        except (sqlite3.Error, OSError):
            page = None
        GLib.idle_add(self.apply_more, page)

    def apply_more(self, page):
        self.refresh_busy = False
        if self.refresh_force:
            self.start_refresh()  # the filters changed: this page belongs to the old list
            return GLib.SOURCE_REMOVE
        if page is None:
            self.cursor = None  # an unreadable page ends the list rather than retrying forever
            self.toasts.add_toast(Adw.Toast(title="Could not read older events"))
        else:
            self.cursor = page["cursor"]
            self.pages_loaded += 1
            self.loaded_events += sum(r["count"] for r in page["rows"])
            self.loaded_rows += len(page["rows"])
            self.fill_list(page["rows"], replace=False)
        self.update_footer()
        return GLib.SOURCE_REMOVE

    def update_footer(self):
        s = self.last_summary
        if s is None:
            return
        more = ""
        if self.cursor is not None:
            more = "  ·  older events load as you scroll" if self.loaded_rows < MAX_ROWS else f"  ·  newest {self.loaded_rows} shown, narrow the range for more"
        self.footer.set_label(f"{self.loaded_events} events{more}  ·  {s['errors_24h']} errors, {s['warnings_24h']} warnings in 24 h  ·  {s['db_bytes'] / 1e6:.1f} MB")

    # ---- building the rows

    def fill_list(self, rows, replace):
        """Builds the rows a chunk per idle callback: hundreds at once is a visible stall."""
        if replace:
            self.list.remove_all()
            self.pending.clear()
        self.pending.extend(rows)
        self.fill_chunk()
        if self.pending and not self.fill_active:
            self.fill_active = True
            GLib.idle_add(self.fill_idle, priority=GLib.PRIORITY_LOW)

    def fill_chunk(self):
        for _ in range(FILL_CHUNK):
            if not self.pending:
                break
            self.list.append(EventRow(self.pending.popleft()))
        self.sync_selection()

    def fill_idle(self):
        self.fill_chunk()
        if self.pending:
            return GLib.SOURCE_CONTINUE
        self.fill_active = False
        self.on_list_scroll(self.list_scroll.get_vadjustment())  # a short list may still need a page
        return GLib.SOURCE_REMOVE

    def update_ages(self):
        i = 0
        while (row := self.list.get_row_at_index(i)) is not None:
            row.update_age()
            i += 1

    def sync_selection(self):
        if self.selected is None:
            self.list.unselect_all()
            return
        index = 0
        while (row := self.list.get_row_at_index(index)) is not None:
            if (row.event["source"], row.event["ref_id"]) == self.selected:
                self.list.select_row(row)
                return
            index += 1
        self.list.unselect_all()
