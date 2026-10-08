"""The overview page."""
import time
from gtkenv import Adw, Gtk
import data
from consts import CHARTS, ICON, LABEL
from fmt import ago, local_epoch, readable
from widgets import event_icon, spec_card, tile


def power_tile(p):
    """CPU watts as the value; the battery's state underneath."""
    if p is None:
        return tile("Power", "–", "no power samples yet")
    watts = "–" if p["cpu_watt"] is None else f"{p['cpu_watt']:.1f} W"
    if p["on_battery"] is None:
        state = "CPU package"
    elif p["on_battery"]:
        draw = f", {p['battery_watt']:.0f} W drawn" if p["battery_watt"] is not None else ""
        state = f"battery {p['battery_pct']:.0f}%{draw}" if p["battery_pct"] is not None else "on battery"
    else:
        state = f"on AC, battery {p['battery_pct']:.0f}%" if p["battery_pct"] is not None else "on AC"
    return tile("Power", watts, state if p["cpu_watt"] is not None else f"CPU counter unreadable · {state}")


def temperature_tile(sensors, gpu):
    """CPU package temperature as the value; the other components underneath."""
    temps = {s["name"]: s["value"] for s in sensors if s["kind"] == "temp"}
    cpu = next((v for n, v in temps.items() if n.startswith("coretemp/Package id")), None)
    nvme = max((v for n, v in temps.items() if n.startswith("nvme") and n.endswith("/Composite")), default=None)
    ram = max((v for n, v in temps.items() if n.startswith("spd5118")), default=None)
    parts = []
    if ram is not None:
        parts.append(f"RAM {ram:.0f} °C")
    if nvme is not None:
        parts.append(f"NVMe {nvme:.0f} °C")
    if gpu and gpu["gpu_temp"] is not None:
        parts.append(f"GPU {gpu['gpu_temp']:.0f} °C")
    return tile("Temperature", "–" if cpu is None else f"{cpu:.0f} °C", " · ".join(parts) or "CPU package")


UNIT = {"temp": "°C", "fan": "rpm", "power": "W"}
# which component a sensor belongs to, by the start of its name; first match wins
COMPONENTS = [
    ("Processor", ("coretemp/", "rapl/core", "rapl/uncore", "rapl/package")),
    ("Memory modules", ("spd5118", "rapl/dram")),
    ("Disks", ("nvme",)),
    ("Platform", ("rapl/psys", "acpitz/")),
    ("Wi-Fi", ("iwlwifi",)),
    ("Fans", ("acpi_fan",)),
]


def component(sensor):
    for title, prefixes in COMPONENTS:
        if sensor["name"].startswith(prefixes):
            return title
    return "Other"


def sensors_group(sensors):
    """Every sensor of the newest sample by component, collapsed: it is the whole of what is logged."""
    group = Adw.PreferencesGroup(title="Everything being logged", description=f"{len(sensors)} readings in the newest sample.")
    titles = [t for t, _ in COMPONENTS] + ["Other"]
    for title in titles:
        found = [s for s in sensors if component(s) == title]
        if not found:
            continue
        expander = Adw.ExpanderRow(title=title, subtitle=f"{len(found)} reading" + ("" if len(found) == 1 else "s"))
        for s in found:
            digits = 1 if s["kind"] == "power" else 0
            row = Adw.ActionRow(title=s["name"], use_markup=False)
            row.add_suffix(Gtk.Label(label=f"{s['value']:.{digits}f} {UNIT[s['kind']]}", css_classes=["dim-label"]))
            expander.add_row(row)
        group.add(expander)
    return group


class OverviewMixin:
    def show_overview(self):
        self.current = None
        self.selected = None
        self.list.unselect_all()
        self.copy_button.set_visible(False)
        self.overview_button.set_visible(False)
        self.content_page.set_title("Overview")
        if self.last_overview is not None:
            self.render_overview(*self.last_overview)  # the cached page, until fresh numbers arrive
        self.stack.set_visible_child_name("overview")
        self.refresh(force=True)

    def render_overview(self, page, s):
        self.last_overview = (page, s)
        self.overview_at = time.monotonic()
        ov, latest, gpu, power, sensors = page["overview"], page["latest"], page["gpu"], page["power"], page["sensors"]
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=20, margin_top=24, margin_bottom=24, margin_start=18, margin_end=18)
        head = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        head.append(Gtk.Label(label="Last 24 hours", xalign=0, css_classes=["title-1"]))
        head.append(Gtk.Label(label="Pick an event, or go to any moment with the clock button.", xalign=0, wrap=True, css_classes=["dim-label"]))
        box.append(head)

        tiles = Gtk.FlowBox(selection_mode=Gtk.SelectionMode.NONE, homogeneous=True, min_children_per_line=2,
                            max_children_per_line=3, column_spacing=12, row_spacing=12)
        last = s["last_sample"] or {}
        pct = lambda v: "–" if v is None else f"{v:.0f}%"  # noqa: E731
        tiles.append(tile("Errors", str(s["errors_24h"]), "last 24 h", ICON["error"], "error"))
        tiles.append(tile("Warnings", str(s["warnings_24h"]), "last 24 h", ICON["warning"], "warning"))
        tiles.append(tile("CPU", pct(last.get("cpu_pct")), f"load {last['load_avg']:.1f}" if last.get("load_avg") is not None else None))
        tiles.append(tile("Memory", pct(last.get("mem_pct")), "in use"))
        tiles.append(tile("GPU", pct(gpu and gpu["gpu_pct"]), f"{gpu['gpu_temp']:.0f} °C" if gpu and gpu["gpu_temp"] is not None else "asleep or absent"))
        tiles.append(power_tile(power))
        tiles.append(temperature_tile(sensors, gpu))
        size = s["db_bytes"] / 1e6
        tiles.append(tile("Recorded", f"{size:.0f} MB" if size >= 10 else f"{size:.1f} MB", "on disk"))
        for child in list(tiles):
            child.set_focusable(False)
        box.append(tiles)

        now = time.time()
        start = max(now - 86400, data.epoch(ov["load"][0]["ts"])) if ov["load"] else now - 86400
        ticks = [(data.epoch(e["ts"]), readable(e["summary"])) for e in ov["events"]]
        every = ov["bucket_s"]
        per = f"{every // 60}-minute" if every >= 60 else f"{every}-second"
        span = "last 24 hours" if now - start > 86000 else f"since {local_epoch(start, '%H:%M')}"
        rows_for = {"usage": ov["load"], "temperature": ov["load"], "power": ov["power"]}
        for spec in CHARTS:
            box.append(spec_card(spec, f"{spec['title']}, {span} · {per} averages", rows_for[spec["name"]], start, now, ticks))

        if sensors:
            box.append(sensors_group(sensors))

        group = Adw.PreferencesGroup(title="Latest problems")
        if not latest:
            group.set_description("Nothing to report.")
        for m in latest:
            row = Adw.ActionRow(title=readable(m["summary"]), subtitle=f"{LABEL.get(m['source'], m['source'])} · {ago(m['ts'])}",
                                use_markup=False, activatable=True, title_lines=2)
            row.add_prefix(event_icon(m))
            if m["count"] > 1:
                row.add_suffix(Gtk.Label(label=f"×{m['count']}", css_classes=["pill"], valign=Gtk.Align.CENTER))
            row.add_suffix(Gtk.Image(icon_name="go-next-symbolic", css_classes=["dim-label"]))
            row.connect("activated", lambda _r, e=m: self.show_detail(e))
            group.add(row)
        box.append(group)
        self.overview_scroll.set_child(self.page(box))
