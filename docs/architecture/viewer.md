# Viewer (`ui/`)

A native GTK4 and libadwaita app, `io.github.hawkosm.Blackbox`, in the application menu. It is
read-only, refreshes every 5 s, and runs only while its window is open. The layout is an adaptive
split view that collapses to single pages below 720 px.

## Modules

| Module | Job |
|---|---|
| `blackbox_app.py` | entry point |
| `window.py` | the window, built from the mixins `sidebar.py`, `overview.py`, `detail.py`, `refresh.py` |
| `widgets.py`, `chart.py` | widgets and charts |
| `fmt.py` | text helpers |
| `consts.py` | labels, colours, stylesheet |
| `gtkenv.py` | pins the GTK versions |
| `data.py` | read-only queries |

## Reading data

- The list and overview are read on a worker thread and applied on the main thread, one read at a
  time.
- The event list is paged: a new filter or time range reads the newest 200 rows, and older pages are
  read as the list scrolls towards its end. It stops at 1,000 rows (the footer says so).
- Rows are built 30 per idle callback.
- Every query is cancelled after 8 s, so a huge database cannot freeze the window.
- While older pages load, the 5 s timer leaves the list alone.

## Views

- **Event list.** Grouped by day. Filters: Problems / Errors / Everything, sources, time range, and
  search. Repeats of the same message fold into one row with a count (`×218`), even with other
  events between them (within 10 minutes) and when only a pid differs.
- **Overview.** Error and warning counts; current CPU, memory, GPU and power (CPU watts, battery
  state); three charts with a tick for every notable event: usage (CPU, memory, GPU, %), temperature
  (GPU, °C) and power (CPU, GPU, battery draw, W).
- **One event.** The same three charts at ±2 min, ±10 min or ±1 h (each with a table view), the
  crash's stack trace from systemd-coredump, the full row with readable field names, and every event
  around it with its offset. Copy puts all of it on the clipboard as a plain-text report.
- **Go to a time.** The clock button opens the same view for any moment, even one where nothing was
  recorded (a freeze, for example).

Each chart has one axis in one unit, with fixed hues for CPU, memory (and battery) and GPU, checked
for colour-blind separation against both the light and dark card surface.
