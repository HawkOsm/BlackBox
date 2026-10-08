"""Labels, colours, text and the stylesheet the viewer is built from."""



APP_ID = "io.github.hawkosm.Blackbox"
SOURCES = [
    ("custom", "Processes"),
    ("journald", "Journal"),
    ("auditd", "Audit"),
    ("sysstat", "System"),
    ("boot", "Boots"),
    ("packages", "Changes"),
]
LABEL = dict(SOURCES)
PAGE_ROWS = 200  # rows read from the database per page
MAX_ROWS = 1000  # the most events the list ever holds, however large the database
SCOPES = [
    ("problems", "Problems", {"error", "warning"}),
    ("errors", "Errors", {"error"}),
    ("all", "Everything", {"error", "warning", "info"}),
]
RANGES = [("Last hour", 1), ("Last 6 hours", 6), ("Last 24 hours", 24), ("Last 7 days", 168), ("All time", None)]
WINDOWS = [("2m", "±2 min", 120), ("10m", "±10 min", 600), ("1h", "±1 h", 3600)]
ICON = {"error": "dialog-error-symbolic", "warning": "dialog-warning-symbolic", "info": "dialog-information-symbolic"}
TONE = {"error": "error", "warning": "warning", "info": "dim-label"}
MOMENT_ICON = "document-open-recent-symbolic"

# Chart series, one colour per source so it means the same thing on every chart: CPU blue, memory
# and battery orange, GPU green. Light and dark steps of the same three hues, validated for
# colour-blind separation against the card surface in both modes. Purple (NVMe, platform power)
# was added afterwards and has not been through that check.
SERIES = {
    "cpu_pct": ("CPU", "#2a78d6", "#3987e5"),
    "mem_pct": ("Memory", "#eb6834", "#d95926"),
    "gpu_pct": ("GPU", "#1baf7a", "#199e70"),
    "gpu_temp": ("GPU", "#1baf7a", "#199e70"),
    "cpu_watt": ("CPU", "#2a78d6", "#3987e5"),
    "gpu_watt": ("GPU", "#1baf7a", "#199e70"),
    "battery_watt": ("Battery draw", "#eb6834", "#d95926"),
    "cpu_temp": ("CPU", "#2a78d6", "#3987e5"),
    "nvme_temp": ("NVMe", "#a24ec0", "#bc73d6"),
    "ram_temp": ("RAM", "#eb6834", "#d95926"),
    "platform_watt": ("Platform", "#a24ec0", "#bc73d6"),
}

# The three charts, in page order. `ymax` None means "fit the data"; `columns` feed the table view.
CHARTS = [
    {"name": "usage", "title": "Usage", "unit": "%", "ymax": 100,
     "series": ("cpu_pct", "mem_pct", "gpu_pct"), "empty": "No system samples here. Was the collector running?",
     "columns": [("cpu %", "cpu_pct", 0), ("mem %", "mem_pct", 0), ("load", "load_avg", 1), ("gpu %", "gpu_pct", 0)]},
    {"name": "temperature", "title": "Temperature", "unit": "°C", "ymax": None,
     "series": ("cpu_temp", "gpu_temp", "nvme_temp", "ram_temp"), "empty": "No temperature readings here. The GPU reports none while it sleeps.",
     "columns": [("cpu °C", "cpu_temp", 0), ("gpu °C", "gpu_temp", 0), ("nvme °C", "nvme_temp", 0), ("ram °C", "ram_temp", 0)]},
    {"name": "power", "title": "Power", "unit": "W", "ymax": None,
     "series": ("cpu_watt", "platform_watt", "gpu_watt", "battery_watt"),
     "empty": "No power readings here. CPU watts need deploy/setup-sensors.sh; battery draw only shows on battery.",
     "columns": [("cpu W", "cpu_watt", 1), ("plat W", "platform_watt", 1), ("gpu W", "gpu_watt", 1), ("batt W", "battery_watt", 1), ("batt %", "battery_pct", 0)]},
]

FIELDS = {
    "comm": "Process",
    "exe": "Executable",
    "pid": "PID",
    "ppid": "Parent PID",
    "exit_code": "How it ended",
    "priority": "Priority",
    "unit": "Unit",
    "message": "Message",
    "event_type": "Audit event",
    "uid": "User ID",
    "executable": "Executable",
    "cpu_pct": "CPU",
    "mem_pct": "Memory",
    "disk_io_kb": "Disk I/O",
    "load_avg": "Load average",
    "gpu_pct": "GPU",
    "gpu_mem_pct": "GPU memory",
    "gpu_temp": "GPU temperature",
    "kind": "What happened",
    "kernel": "Kernel",
    "kernel_boot": "Boot ID",
    "note": "Last thing it logged",
    "command": "Command",
}
KINDS = {"start": "The system started", "unclean_end": "The system stopped without a clean shutdown"}
SIGNALS = {1: "SIGHUP", 2: "SIGINT", 3: "SIGQUIT", 4: "SIGILL", 5: "SIGTRAP", 6: "SIGABRT", 7: "SIGBUS",
           8: "SIGFPE", 9: "SIGKILL", 11: "SIGSEGV", 13: "SIGPIPE", 15: "SIGTERM", 31: "SIGSYS"}
PRIORITIES = ["emergency", "alert", "critical", "error", "warning", "notice", "info", "debug"]

CSS = """
.tile { padding: 14px 16px; }
.tile-value { font-size: 1.9em; font-weight: 800; font-feature-settings: "tnum"; }
.pill { border-radius: 999px; padding: 1px 8px; background: alpha(currentColor, 0.1);
        font-weight: 700; font-size: 0.8em; font-feature-settings: "tnum"; }
.day-header { font-weight: 700; font-size: 0.8em; opacity: 0.6; padding: 14px 12px 4px 12px; }
.event-title { font-weight: 600; }
.chart-card { padding: 14px 16px 10px 16px; }
.swatch { min-width: 10px; min-height: 10px; border-radius: 3px; }
.mono { font-family: monospace; font-size: 0.85em; }
"""

# one swatch class per series, light and dark
for _key, (_label, _light, _dark) in SERIES.items():
    CSS += f".swatch-{_key} {{ background: {_light}; }}\n"
CSS += "@media (prefers-color-scheme: dark) {\n"
for _key, (_label, _light, _dark) in SERIES.items():
    CSS += f"  .swatch-{_key} {{ background: {_dark}; }}\n"
CSS += "}\n"
