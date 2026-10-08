"""One place that pins the GTK versions, so importing any viewer module just works."""
import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
gi.require_version("PangoCairo", "1.0")
from gi.repository import Adw, Gdk, Gio, GLib, GObject, Gtk, Pango, PangoCairo  # noqa: E402,F401
