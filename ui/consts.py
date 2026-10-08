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
RANGES = [("1 h", 1), ("6 h", 6), ("24 h", 24), ("7 days", 168), ("All", None)]
GLANCE_TITLE = {1: "The last hour", 6: "The last 6 hours", 24: "The day", 168: "The week", None: "Everything"}
SIZE_CAP = "50 GB"  # the recorder's default BLACKBOX_MAX_MB; the viewer cannot see an override
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

# The palette is dark only: the app forces the dark scheme (see blackbox_app.py).
CSS = """
:root {
  --window-bg-color: #0A0A0A; --window-fg-color: #E4E4E6;
  --view-bg-color: #0A0A0A; --view-fg-color: #E4E4E6;
  --headerbar-bg-color: #0A0A0A; --headerbar-fg-color: #E4E4E6; --headerbar-shade-color: #1C1C21;
  --headerbar-backdrop-color: #0A0A0A;
  --sidebar-bg-color: #0E0E10; --sidebar-fg-color: #E4E4E6; --sidebar-backdrop-color: #0E0E10;
  --sidebar-shade-color: #1C1C21; --sidebar-border-color: #1C1C21;
  --card-bg-color: #0E0E10; --card-fg-color: #E4E4E6; --card-shade-color: #1C1C21;
  --popover-bg-color: #1C1C21; --popover-fg-color: #E4E4E6;
  --dialog-bg-color: #0E0E10; --thumbnail-bg-color: #1C1C21;
  --border-color: #1C1C21;
  --error-color: #EF4444; --error-bg-color: #EF4444; --warning-color: #F2B33D; --warning-bg-color: #F2B33D;
  --success-color: #22C55E;
  --accent-bg-color: #3A3A49; --accent-fg-color: #FFFFFF; --accent-color: #D6D9E9;
}
@define-color window_bg_color #0A0A0A; @define-color window_fg_color #E4E4E6;
@define-color view_bg_color #0A0A0A; @define-color view_fg_color #E4E4E6;
@define-color headerbar_bg_color #0A0A0A; @define-color headerbar_fg_color #E4E4E6;
@define-color sidebar_bg_color #0E0E10; @define-color sidebar_fg_color #E4E4E6;
@define-color card_bg_color #0E0E10; @define-color card_fg_color #E4E4E6;
@define-color popover_bg_color #1C1C21; @define-color popover_fg_color #E4E4E6;
@define-color dialog_bg_color #0E0E10; @define-color dialog_fg_color #E4E4E6;
@define-color accent_bg_color #3A3A49; @define-color accent_fg_color #FFFFFF; @define-color accent_color #D6D9E9;
@define-color theme_fg_color #E4E4E6; @define-color theme_text_color #E4E4E6;
@define-color theme_bg_color #0A0A0A; @define-color theme_base_color #0A0A0A;
@define-color headerbar_border_color #1C1C21; @define-color headerbar_backdrop_color #0A0A0A; @define-color headerbar_shade_color #1C1C21;
@define-color card_shade_color #1C1C21; @define-color shade_color rgba(0, 0, 0, 0.36); @define-color borders #1C1C21;
@define-color error_color #EF4444; @define-color error_bg_color #EF4444; @define-color destructive_color #EF4444; @define-color destructive_bg_color #EF4444;
@define-color warning_color #F2B33D; @define-color warning_bg_color #F2B33D; @define-color success_color #22C55E; @define-color success_bg_color #22C55E;
@define-color theme_unfocused_bg_color #0A0A0A; @define-color theme_unfocused_base_color #0A0A0A; @define-color theme_unfocused_fg_color #E4E4E6;
@define-color theme_selected_bg_color #3A3A49; @define-color theme_selected_fg_color #FFFFFF;
window, popover, label, entry, button { font-family: "General Sans", sans-serif; }
window, popover { color: #E4E4E6; }
.heading, .title-1, .title-2, .title-3 { color: #FFFFFF; }
.card { background: #0E0E10; border: 1px solid #1C1C21; border-radius: 8px; box-shadow: none; }
.tnum, .num { font-feature-settings: "tnum"; }

/* sidebar */
.sidebar-pane { background: #0E0E10; border-right: 1px solid #1C1C21; }
.sidebar-pane headerbar { background: #0E0E10; box-shadow: none; min-height: 0; padding: 12px 14px 6px 14px; }
.search-field { background: #1C1C21; border-radius: 8px; min-height: 34px; padding: 0 12px; }
.search-field entry { background: none; box-shadow: none; outline: none; min-height: 34px; padding: 0; font-size: 13px; }
.search-field entry > image:first-child { opacity: 0; -gtk-icon-size: 1px; min-width: 0; margin: 0; padding: 0; }
.search-field entry text > placeholder { color: #62646C; }
.key-hint { font-size: 11px; padding: 1px 6px; border-radius: 4px; border: 1px solid #3A3A49; color: #62646C; }
.filter-button > button { background: #1C1C21; border-radius: 8px; min-width: 34px; min-height: 34px; padding: 0; }
button:active, .filter-button > button:active { transform: scale(0.95); }

.segmented { background: #0A0A0A; border: 1px solid #1C1C21; border-radius: 8px; padding: 3px; }
.segmented toggle, .segmented > button { background: none; box-shadow: none; border-radius: 6px; padding: 5px 0; min-height: 0;
  font-size: 12.5px; font-weight: 500; color: #AFB0B6; margin: 0 1px; }
.segmented toggle:checked { background: #1C1C21; color: #FFFFFF; }
.range-switch { background: #0E0E10; }
.range-switch toggle { padding: 4px 10px; font-weight: 400; }
.range-switch toggle:checked { font-weight: 500; }
.scope-switch { margin: 0 14px 10px 14px; }

.event-list { background: none; padding: 6px 8px; }
.event-list > row { padding: 10px 12px; border-radius: 8px; margin-bottom: 2px; background: none; }
.event-list > row:hover { background: alpha(#1C1C21, 0.5); }
.event-list > row:selected { background: #1C1C21; }
.sev-dot { min-width: 8px; min-height: 8px; border-radius: 4px; margin-top: 6px; }
.sev-error { background: #EF4444; }
.sev-warning { background: #F2B33D; }
.sev-info { background: #D6D9E9; }
.ev-name { font-size: 14px; font-weight: 600; color: #FFFFFF; }
.ev-desc { font-size: 13px; color: #AFB0B6; }
.ev-detail { font-size: 12px; color: #62646C; }
.ev-time { font-size: 11px; color: #62646C; font-feature-settings: "tnum"; }
.ev-count { font-size: 12px; font-weight: 600; color: #E4E4E6; font-feature-settings: "tnum"; }
.sidebar-footer { border-top: 1px solid #1C1C21; padding: 10px 18px; font-size: 11.5px; color: #62646C; }

/* content */
.content-pane, .content-pane stack { background: #0A0A0A; }
.content-header:backdrop { background: #0A0A0A; }
.content-header { min-height: 52px; padding: 0 14px; box-shadow: inset 0 -1px #1C1C21; background: #0A0A0A; }
.moment-button > button { padding: 7px 12px; border-radius: 6px; background: #1C1C21; font-size: 13px; font-weight: 400; min-height: 0; }
.glance-card { padding: 16px 22px 12px 22px; }
.card-title { font-size: 14px; font-weight: 600; color: #FFFFFF; }
.legend-item { font-size: 11.5px; color: #AFB0B6; }
.legend-swatch { min-width: 8px; min-height: 8px; border-radius: 2px; }

/* processes */
.proc-head { padding: 12px 12px 12px 22px; border-bottom: 1px solid #1C1C21; }
.meta { font-size: 12px; color: #62646C; }
.act { padding: 6px 12px; border-radius: 6px; background: #1C1C21; font-size: 12.5px; font-weight: 400; min-height: 0; }
.act:disabled { opacity: 0.4; }
.act.danger { color: #EF4444; }
.proc-columns { padding: 0 12px; }
.proc-columns > button { padding: 9px 10px 7px 10px; font-size: 11.5px; font-weight: 500; color: #62646C; background: none;
  border-radius: 0; min-height: 0; box-shadow: none; }
.proc-columns > button.sorted { color: #FFFFFF; box-shadow: inset 0 -2px #E4E4E6; }
.proc-list { background: none; padding: 0 12px 8px 12px; }
.proc-list > row { min-height: 34px; padding: 0; border-radius: 6px; margin-bottom: 2px; font-size: 12.5px; background: none; }
.proc-list > row:hover { background: alpha(#1C1C21, 0.5); }
.proc-list > row:selected { background: #1C1C21; }
.proc-name { color: #FFFFFF; padding-left: 10px; }
.proc-count { font-size: 11px; color: #62646C; }
.proc-tag { font-size: 11px; color: #AFB0B6; }
.proc-pid { color: #62646C; padding: 0 10px; font-feature-settings: "tnum"; }
.num { padding: 0 10px; border-radius: 4px; color: #E4E4E6; }
.status { padding: 0 10px; color: #AFB0B6; }
.status.stopped { color: #F2B33D; }
.status.ending { color: #EF4444; }

/* the detail page, still on the stock widgets */
.pill { border-radius: 999px; padding: 1px 8px; background: alpha(currentColor, 0.1);
        font-weight: 700; font-size: 0.8em; font-feature-settings: "tnum"; }
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

# Heat tints for the process list's number cells, in eight steps: alpha = min(0.32, 0.04 + v/max × 0.28).
HEAT = {"cpu": ((57, 135, 229), 15), "mem": ((217, 89, 38), 3000), "gpu": ((25, 158, 112), 15), "disk": ((188, 115, 214), 2)}
for _name, ((_r, _g, _b), _max) in HEAT.items():
    for _step in range(1, 9):
        CSS += f".heat-{_name}-{_step} {{ background: rgba({_r}, {_g}, {_b}, {0.04 + _step / 8 * 0.28:.3f}); }}\n"
