// OS Monitor backend — a thin pass-through over ctx.resource.sysinfo.
//
// Every verb is a live read: the edge samples the host and records nothing, so
// this app persists no state and replay stays trivial. The UI polls `snapshot`
// on a timer; the granular verbs exist for the CLI and for agents.

var sys = ctx.resource && ctx.resource.sysinfo;

var description =
  "Live system monitor — CPU, memory, disk, network, battery, and top " +
  "processes — reading ctx.resource.sysinfo.";

function ungranted() {
  return (
    "sysinfo not granted. Grant it with:\n" +
    "  terrane auth grant user:local-owner os-monitor sysinfo"
  );
}

// Build one no-arg action that forwards to a sysinfo read of the same name.
function section(name, summary) {
  return {
    summary: summary,
    args: [],
    returns: "a JSON document",
    run: function () {
      if (!sys) return ungranted();
      return sys[name]();
    },
  };
}

var actions = {
  snapshot: section(
    "snapshot",
    "Full metrics snapshot as JSON — every section in one read (what the UI polls)."
  ),
  cpu: section("cpu", "CPU usage overall and per-core, plus load average."),
  memory: section("memory", "RAM and swap usage."),
  disk: section("disk", "Per-volume disk capacity."),
  network: section("network", "Network throughput and cumulative totals."),
  battery: section("battery", "Battery / power state (best-effort)."),
  system: section("system", "Host and OS identity plus uptime."),

  processes: {
    summary: "Top processes as JSON, ranked by CPU or memory.",
    args: [
      {
        name: "sortBy",
        required: false,
        summary: '"cpu" (default) or "memory"',
      },
      { name: "limit", required: false, summary: "max rows (default 8)" },
    ],
    returns: 'a JSON object {"sortBy","processes":[{pid,name,cpu,memory}]}',
    run: function (args) {
      if (!sys) return ungranted();
      var sortBy = args[0] || "cpu";
      var limit = args[1] || "8";
      return sys.processes(sortBy, limit);
    },
  },
};
