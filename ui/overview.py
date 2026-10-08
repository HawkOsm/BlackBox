"""The overview page: one chart of the chosen range, and the running processes."""
import time
from gtkenv import Gtk
from chart import GlanceChart
from consts import GLANCE_TITLE


def legend():
    """CPU, memory, GPU and the purple NVMe / platform pair, as the lanes colour them."""
    box = Gtk.Box(spacing=14, valign=Gtk.Align.CENTER)
    for key, label in (("cpu_pct", "CPU"), ("mem_pct", "Memory"), ("gpu_pct", "GPU"), ("nvme_temp", "NVMe / platform")):
        item = Gtk.Box(spacing=6)
        swatch = Gtk.Box(css_classes=["legend-swatch", f"swatch-{key}"], valign=Gtk.Align.CENTER)
        item.append(swatch)
        item.append(Gtk.Label(label=label, css_classes=["legend-item"]))
        box.append(item)
    return box


class OverviewMixin:
    def build_overview(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16, margin_top=20, margin_bottom=20, margin_start=24, margin_end=24)
        card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12, css_classes=["card", "glance-card"])
        head = Gtk.Box(spacing=12)
        self.glance_title = Gtk.Label(label=f"{GLANCE_TITLE[self.hours]} at a glance", xalign=0, hexpand=True, css_classes=["card-title"])
        head.append(self.glance_title)
        head.append(legend())
        card.append(head)
        self.glance = GlanceChart()
        card.append(self.glance)
        box.append(card)
        box.append(self.build_processes())
        return box

    def show_overview(self):
        self.current = None
        self.selected = None
        self.list.unselect_all()
        self.copy_button.set_visible(False)
        self.overview_button.set_visible(False)
        self.content_page.set_title("Overview")
        self.stack.set_visible_child_name("overview")
        self.refresh(force=True)
        self.poll_processes()

    def render_overview(self, page):
        self.overview_at = time.monotonic()
        self.glance_title.set_label(f"{GLANCE_TITLE[self.hours]} at a glance")
        self.glance.set_data(page["glance"])
        self.crash_tags = page["crashes"]

