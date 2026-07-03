//! Live host metrics — the edge side of the `sysinfo` capability's reads.
//!
//! `EdgeRunner` implements [`LiveHost`](terrane_core::LiveHost) by delegating to
//! [`sample`], which reads the current machine state and returns it as JSON.
//! Nothing here is recorded: these are live reads, invoked at read time, so they
//! never enter the event log and replay is unaffected.
//!
//! Rate-based fields (CPU %, network throughput) are deltas between consecutive
//! refreshes, so the sampler is a process-global [`Monitor`] that outlives any
//! single short-lived `EdgeRunner`. The first sample after start reports zero
//! rates, exactly as a freshly launched system monitor would.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde_json::{json, Value};
use sysinfo::{Disks, Networks, ProcessesToUpdate, System};
use terrane_core::{Error, Result};

/// Process-global sampler shared across every read.
struct Monitor {
    sys: System,
    disks: Disks,
    networks: Networks,
    /// When the network counters were last read, for converting byte deltas to
    /// a per-second rate.
    last_net: Option<Instant>,
}

fn monitor() -> &'static Mutex<Monitor> {
    static M: OnceLock<Mutex<Monitor>> = OnceLock::new();
    M.get_or_init(|| {
        Mutex::new(Monitor {
            sys: System::new(),
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
            last_net: None,
        })
    })
}

/// Sample one metric `domain` and return it as a JSON document string. Mirrors
/// the `ctx.resource.sysinfo.*` method names declared by `terrane-cap-sysinfo`.
pub fn sample(domain: &str, args: &[String]) -> Result<String> {
    let mut m = monitor().lock().unwrap_or_else(|e| e.into_inner());
    let value = match domain {
        "snapshot" => m.snapshot(args),
        "cpu" => m.cpu(),
        "memory" => m.memory(),
        "disk" => m.disk(),
        "network" => m.network(),
        "battery" => battery(),
        "system" => m.system(),
        "processes" => m.processes(args),
        other => {
            return Err(Error::InvalidInput(format!(
                "unknown sysinfo domain: {other}"
            )))
        }
    };
    serde_json::to_string(&value).map_err(|e| Error::Runtime(format!("sysinfo serialize: {e}")))
}

impl Monitor {
    fn snapshot(&mut self, _args: &[String]) -> Value {
        json!({
            "cpu": self.cpu(),
            "memory": self.memory(),
            "disk": self.disk(),
            "network": self.network(),
            "battery": battery(),
            "system": self.system(),
            "processes": self.processes(&[]),
        })
    }

    fn cpu(&mut self) -> Value {
        self.sys.refresh_cpu_all();
        let per_core: Vec<Value> = self
            .sys
            .cpus()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                json!({
                    "core": i,
                    "name": c.name(),
                    "usage": round2(c.cpu_usage() as f64),
                    "frequencyMhz": c.frequency(),
                })
            })
            .collect();
        let load = System::load_average();
        json!({
            "usage": round2(self.sys.global_cpu_usage() as f64),
            "cores": self.sys.cpus().len(),
            "brand": self.sys.cpus().first().map(|c| c.brand().trim().to_string()),
            "perCore": per_core,
            "load": {
                "one": round2(load.one),
                "five": round2(load.five),
                "fifteen": round2(load.fifteen),
            },
        })
    }

    fn memory(&mut self) -> Value {
        self.sys.refresh_memory();
        let total = self.sys.total_memory();
        let used = self.sys.used_memory();
        let swap_total = self.sys.total_swap();
        let swap_used = self.sys.used_swap();
        json!({
            "total": total,
            "used": used,
            "free": self.sys.free_memory(),
            "available": self.sys.available_memory(),
            "usage": pct(used, total),
            "swapTotal": swap_total,
            "swapUsed": swap_used,
            "swapUsage": pct(swap_used, swap_total),
        })
    }

    fn disk(&mut self) -> Value {
        self.disks.refresh(true);
        let volumes: Vec<Value> = self
            .disks
            .list()
            .iter()
            .map(|d| {
                let total = d.total_space();
                let free = d.available_space();
                let used = total.saturating_sub(free);
                json!({
                    "name": d.name().to_string_lossy(),
                    "mountPoint": d.mount_point().to_string_lossy(),
                    "fileSystem": d.file_system().to_string_lossy(),
                    "removable": d.is_removable(),
                    "total": total,
                    "used": used,
                    "free": free,
                    "usage": pct(used, total),
                })
            })
            .collect();
        json!({ "volumes": volumes })
    }

    fn network(&mut self) -> Value {
        self.networks.refresh(true);
        let elapsed = self
            .last_net
            .map(|t| t.elapsed().as_secs_f64())
            .filter(|s| *s > 0.0);
        self.last_net = Some(Instant::now());

        let mut rx = 0u64;
        let mut tx = 0u64;
        let mut total_down = 0u64;
        let mut total_up = 0u64;
        let mut interfaces: Vec<Value> = self
            .networks
            .iter()
            .map(|(name, data)| {
                let r = data.received();
                let t = data.transmitted();
                rx += r;
                tx += t;
                total_down += data.total_received();
                total_up += data.total_transmitted();
                json!({
                    "name": name,
                    "downRate": rate(r, elapsed),
                    "upRate": rate(t, elapsed),
                    "totalDown": data.total_received(),
                    "totalUp": data.total_transmitted(),
                })
            })
            .collect();
        interfaces.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        json!({
            "downRate": rate(rx, elapsed),
            "upRate": rate(tx, elapsed),
            "totalDown": total_down,
            "totalUp": total_up,
            "interfaces": interfaces,
        })
    }

    fn system(&mut self) -> Value {
        json!({
            "hostName": System::host_name(),
            "osName": System::name(),
            "osVersion": System::os_version(),
            "osLongVersion": System::long_os_version(),
            "kernelVersion": System::kernel_version(),
            "arch": System::cpu_arch(),
            "cpuBrand": self.sys.cpus().first().map(|c| c.brand().trim().to_string()),
            "physicalCores": self.sys.physical_core_count(),
            "logicalCores": self.sys.cpus().len(),
            "uptimeSeconds": System::uptime(),
        })
    }

    fn processes(&mut self, args: &[String]) -> Value {
        self.sys
            .refresh_processes(ProcessesToUpdate::All, true);
        let sort_by = args.first().map(String::as_str).unwrap_or("cpu");
        let limit = args
            .get(1)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(8)
            .clamp(1, 100);

        let mut rows: Vec<(u32, String, f32, u64)> = self
            .sys
            .processes()
            .iter()
            .map(|(pid, p)| {
                (
                    pid.as_u32(),
                    p.name().to_string_lossy().into_owned(),
                    p.cpu_usage(),
                    p.memory(),
                )
            })
            .collect();
        if sort_by == "memory" {
            rows.sort_by_key(|row| std::cmp::Reverse(row.3));
        } else {
            rows.sort_by(|a, b| b.2.total_cmp(&a.2));
        }
        rows.truncate(limit);

        let list: Vec<Value> = rows
            .into_iter()
            .map(|(pid, name, cpu, mem)| {
                json!({
                    "pid": pid,
                    "name": name,
                    "cpu": round2(cpu as f64),
                    "memory": mem,
                })
            })
            .collect();
        json!({ "sortBy": sort_by, "processes": list })
    }
}

/// Battery / power state, best-effort. macOS reads `pmset -g batt`; other
/// platforms report `present: false` (no cross-platform battery source here).
fn battery() -> Value {
    #[cfg(target_os = "macos")]
    {
        parse_pmset()
    }
    #[cfg(not(target_os = "macos"))]
    {
        json!({ "present": false })
    }
}

#[cfg(target_os = "macos")]
fn parse_pmset() -> Value {
    use std::process::Command;

    let output = match Command::new("/usr/bin/pmset").args(["-g", "batt"]).output() {
        Ok(o) if o.status.success() => o,
        _ => return json!({ "present": false }),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let source = if text.contains("'AC Power'") {
        "AC Power"
    } else if text.contains("'Battery Power'") {
        "Battery Power"
    } else {
        "Unknown"
    };

    let Some(line) = text.lines().find(|l| l.contains("InternalBattery")) else {
        return json!({ "present": false, "source": source });
    };

    let percent = line.split_whitespace().find_map(|t| {
        t.strip_suffix("%;")
            .or_else(|| t.strip_suffix('%'))
            .and_then(|n| n.parse::<u8>().ok())
    });
    let state = ["charging", "discharging", "charged", "finishing charge", "AC attached"]
        .into_iter()
        .find(|s| line.contains(s));
    let time_remaining = line.split_whitespace().find(|t| {
        t.contains(':') && t.chars().next().is_some_and(|c| c.is_ascii_digit())
    });

    json!({
        "present": true,
        "source": source,
        "percent": percent,
        "state": state,
        "timeRemaining": time_remaining,
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn pct(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        round2(used as f64 / total as f64 * 100.0)
    }
}

/// Bytes-per-second from a byte delta and the elapsed interval, or 0 on the
/// first sample (no prior instant to measure against).
fn rate(bytes: u64, elapsed: Option<f64>) -> u64 {
    match elapsed {
        Some(secs) => (bytes as f64 / secs) as u64,
        None => 0,
    }
}
