"""Running processes, read from /proc and grouped by app, for the process list.

Pure reading, no widgets: `Poller.poll()` runs on a worker thread every couple of seconds. Rates
(CPU, GPU, disk) are the difference from the previous poll, so the first poll shows zeros.

An app is a process plus the descendants that run the same executable (Firefox's content
processes, Electron's helpers, a server's workers). The executable of another user's process
cannot be read, so for those the name stands in for it."""
import os

CLK_TCK = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")
NCPU = os.cpu_count() or 1
DRI = "/dev/dri/"
DRM_RESCAN = 5  # polls between full scans for GPU clients; in between only known clients are read


def read(path):
    try:
        with open(path, "rb") as f:
            return f.read().decode(errors="replace")
    except OSError:
        return None


def stat(pid):
    """(comm, state, ppid, cpu ticks, start time, rss bytes) from /proc/<pid>/stat, or None."""
    text = read(f"/proc/{pid}/stat")
    if text is None:
        return None
    head, _, rest = text.rpartition(")")
    f = rest.split()
    return head.partition("(")[2], f[0], int(f[1]), int(f[11]) + int(f[12]), int(f[19]), int(f[21]) * PAGE


def io_bytes(pid):
    """Bytes this process made the disks read and write; None when not ours to read."""
    text = read(f"/proc/{pid}/io")
    if text is None:
        return None
    fields = dict(line.split(": ") for line in text.splitlines() if ": " in line)
    return int(fields.get("read_bytes", 0)) + int(fields.get("write_bytes", 0))


def drm_fds(pid):
    """File descriptors of this process that are open on a GPU."""
    try:
        names = os.listdir(f"/proc/{pid}/fd")
    except OSError:
        return []
    found = []
    for fd in names:
        try:
            if os.readlink(f"/proc/{pid}/fd/{fd}").startswith(DRI):
                found.append(fd)
        except OSError:
            pass
    return found


def drm_usage(pid, fds):
    """{(device, client id): {engine: busy ns}} from the DRM fdinfo of these descriptors. Two
    descriptors can share a client, so clients are keyed rather than summed per descriptor."""
    clients = {}
    for fd in fds:
        text = read(f"/proc/{pid}/fdinfo/{fd}")
        if not text or "drm-client-id" not in text:
            continue
        info = dict(line.split(":", 1) for line in text.splitlines() if ":" in line)
        key = (info.get("drm-pdev", "").strip(), info["drm-client-id"].strip())
        engines = {k[11:]: int(v.split()[0]) for k, v in info.items()
                   if k.startswith("drm-engine-") and not k.startswith("drm-engine-capacity") and v.strip().endswith("ns")}
        clients[key] = engines
    return clients


class Poller:
    def __init__(self):
        self.prev = {}  # (pid, start time) -> (cpu ticks, io bytes, {client: {engine: ns}})
        self.prev_t = None
        self.polls = 0
        self.gpu_pids = {}  # pid -> drm fds, refreshed every DRM_RESCAN polls

    def poll(self, now):
        """Every app as a dict: pid (of its top process), name, pids, cpu %, mem_mb, gpu %,
        disk_mbs, stopped."""
        elapsed = now - self.prev_t if self.prev_t else None
        rescan = self.polls % DRM_RESCAN == 0
        self.polls += 1
        procs = {}
        for name in os.listdir("/proc"):
            if not name.isdigit():
                continue
            pid = int(name)
            s = stat(pid)
            # kernel threads: kthreadd (2) and its children; they own no memory map either
            if s is None or pid == 2 or s[2] == 2 or s[1] == "Z":
                continue
            comm, state, ppid, ticks, start, rss = s
            try:
                exe = os.readlink(f"/proc/{pid}/exe")
                uid = os.stat(f"/proc/{pid}").st_uid
            except OSError:
                exe, uid = None, None
            if rescan:
                fds = drm_fds(pid)
                if fds:
                    self.gpu_pids[pid] = fds
                else:
                    self.gpu_pids.pop(pid, None)
            gpu = drm_usage(pid, self.gpu_pids[pid]) if pid in self.gpu_pids else {}
            procs[pid] = {"pid": pid, "comm": comm, "state": state, "ppid": ppid, "exe": exe, "uid": uid, "key": (pid, start),
                          "ticks": ticks, "rss": rss, "io": io_bytes(pid), "gpu": gpu}
        for pid in list(self.gpu_pids):
            if pid not in procs:
                del self.gpu_pids[pid]

        # rates against the previous poll
        seen_clients = {}
        for p in procs.values():
            before = self.prev.get(p["key"])
            p["cpu"] = p["disk"] = p["gpu_pct"] = 0.0
            if before and elapsed:
                p["cpu"] = max(0, p["ticks"] - before[0]) / CLK_TCK / elapsed / NCPU * 100
                if p["io"] is not None and before[1] is not None:
                    p["disk"] = max(0, p["io"] - before[1]) / elapsed / 1e6
                for client, engines in p["gpu"].items():
                    if client in seen_clients:
                        continue  # a client shared with another process counts once
                    seen_clients[client] = True
                    old = before[2].get(client, {})
                    busy = [max(0, ns - old.get(e, ns)) / (elapsed * 1e9) * 100 for e, ns in engines.items()]
                    p["gpu_pct"] += max(busy, default=0)
        self.prev = {p["key"]: (p["ticks"], p["io"], p["gpu"]) for p in procs.values()}
        self.prev_t = now

        def same_app(p, parent):
            if p["uid"] != parent["uid"]:
                return False  # a user's systemd is not part of pid 1's, though both run systemd
            if p["exe"] is not None and parent["exe"] is not None:
                return p["exe"] == parent["exe"]
            return p["comm"] == parent["comm"]

        def root(p):
            while (parent := procs.get(p["ppid"])) is not None and same_app(p, parent):
                p = parent
            return p

        apps = {}
        for p in procs.values():
            top = root(p)
            app = apps.get(top["pid"])
            if app is None:
                app = apps[top["pid"]] = {"pid": top["pid"], "name": top["comm"], "exe": top["exe"], "pids": [],
                                          "cpu": 0.0, "mem_mb": 0.0, "gpu": 0.0, "disk_mbs": 0.0,
                                          "stopped": top["state"] in "Tt"}
            app["pids"].append(p["pid"])
            app["cpu"] += p["cpu"]
            app["mem_mb"] += p["rss"] / 1048576
            app["gpu"] += p["gpu_pct"]
            app["disk_mbs"] += p["disk"]
        for app in apps.values():
            app["gpu"] = min(100.0, app["gpu"])
        return list(apps.values())
