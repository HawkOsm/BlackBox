#!/usr/bin/env python3
"""Blackbox: native GTK4/libadwaita viewer for the blackbox database. Runs only while its window is open."""
import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import data  # noqa: E402
from consts import APP_ID, CSS  # noqa: E402
from gtkenv import Adw, Gdk, Gio, GLib, Gtk  # noqa: E402
from window import Window  # noqa: E402


class App(Adw.Application):
    def __init__(self, args):
        flags = Gio.ApplicationFlags.NON_UNIQUE if args.screenshot else Gio.ApplicationFlags.DEFAULT_FLAGS
        super().__init__(application_id=APP_ID, flags=flags)
        self.args = args

    def do_startup(self):
        Adw.Application.do_startup(self)
        provider = Gtk.CssProvider()
        provider.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        if self.args.style:
            scheme = Adw.ColorScheme.FORCE_DARK if self.args.style == "dark" else Adw.ColorScheme.FORCE_LIGHT
            Adw.StyleManager.get_default().set_color_scheme(scheme)

    def do_activate(self):
        win = self.props.active_window
        if win is None:
            win = Window(self, data.Data(self.args.db))
            if self.args.size:
                w, h = (int(v) for v in self.args.size.lower().split("x"))
                win.set_default_size(w, h)
            if self.args.screenshot:
                select = 0 if self.args.select_first else self.args.select
                GLib.timeout_add(1500, lambda: win.snapshot(self.args.screenshot, select) or False)
            if self.args.close_after:
                GLib.timeout_add_seconds(self.args.close_after, lambda: win.close() or False)
            if self.args.screenshot:
                # a headless display has no frame clock, so transitions would never finish
                Gtk.Settings.get_default().set_property("gtk-enable-animations", False)
            # otherwise the first row takes the initial focus, and a focused row gets selected
            win.set_focus(win.search_button)
        win.present()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", default=data.default_db())
    parser.add_argument("--screenshot", help="write a PNG of the window and exit (for testing)")
    parser.add_argument("--close-after", type=int, help="close the window after N seconds (for testing)")
    parser.add_argument("--select-first", action="store_true", help="with --screenshot: open the newest entry first")
    parser.add_argument("--select", type=int, help="with --screenshot: open the entry at this list position")
    parser.add_argument("--style", choices=["light", "dark"], help="force a colour scheme (for testing)")
    parser.add_argument("--size", help="initial window size, e.g. 480x800 (for testing)")
    args = parser.parse_args()
    sys.exit(App(args).run([sys.argv[0]]))


if __name__ == "__main__":
    main()
