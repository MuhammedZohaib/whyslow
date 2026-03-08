mod windows;

use std::collections::{BTreeSet, HashMap};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use thiserror::Error;
use tracing::debug;

use crate::model::{
    CollectionWindow, DiskDeviceSample, DiskSample, HostInfo, MarkerFlags, NetworkSample,
    ProcessSample, RunConfig, Sample,
};

#[derive(Debug, Error)]
pub enum CollectError {
    #[error("no samples were collected during the requested window")]
    NoSamples,
}

/// Stateful collector that can produce one sample at a time.
pub struct LiveCollector {
    system: sysinfo::System,
    disks: sysinfo::Disks,
    networks: sysinfo::Networks,
    windows_collector: windows::WindowsCollector,
    top_n: usize,
    interval_secs: f64,
}

impl LiveCollector {
    pub fn new(top_n: usize, interval_ms: u64) -> Self {
        let mut system = sysinfo::System::new_all();
        system.refresh_all();
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let mut networks = sysinfo::Networks::new_with_refreshed_list();
        networks.refresh();

        Self {
            system,
            disks,
            networks,
            windows_collector: windows::WindowsCollector::new(),
            top_n,
            interval_secs: (interval_ms.max(100) as f64) / 1000.0,
        }
    }

    pub fn warm_up(&self, interval: Duration) {
        thread::sleep(interval);
    }

    pub fn host_info(&self) -> HostInfo {
        HostInfo {
            hostname: sysinfo::System::host_name(),
            os_name: sysinfo::System::name(),
            os_version: sysinfo::System::os_version(),
            kernel_version: sysinfo::System::kernel_version(),
            cpu_core_count: self.system.cpus().len(),
            cpu_logical_core_count: Some(self.system.cpus().len()),
            cpu_physical_core_count: self.system.physical_core_count(),
            total_memory_bytes: Some(self.system.total_memory()),
            uptime_secs: Some(sysinfo::System::uptime()),
        }
    }

    pub fn sample_once(&mut self) -> Sample {
        self.system.refresh_all();
        self.disks.refresh();
        self.networks.refresh();
        build_sample(
            &self.system,
            &self.disks,
            &self.networks,
            self.top_n,
            self.interval_secs,
            &mut self.windows_collector,
        )
    }
}

/// Collects repeated system snapshots over the configured sampling window.
pub fn collect_window(config: &RunConfig) -> Result<CollectionWindow, CollectError> {
    let interval = Duration::from_millis(config.interval_ms.max(100));
    let sample_window = Duration::from_secs(config.sample_window_secs.max(1));

    let mut collector = LiveCollector::new(config.top_n.max(8), config.interval_ms);
    collector.warm_up(interval);

    let mut samples = Vec::new();
    let started_at = Utc::now();
    let deadline = Instant::now() + sample_window;

    while Instant::now() < deadline {
        let sample = collector.sample_once();
        debug!(sample_time = %sample.timestamp, "captured sample");
        samples.push(sample);

        if Instant::now() + interval >= deadline {
            break;
        }
        thread::sleep(interval);
    }

    if samples.is_empty() {
        return Err(CollectError::NoSamples);
    }

    Ok(CollectionWindow {
        started_at,
        ended_at: Utc::now(),
        interval_ms: config.interval_ms,
        host: collector.host_info(),
        unavailable_metrics: unavailable_metrics_from_samples(&samples),
        samples,
    })
}

pub fn unavailable_metrics_from_samples(samples: &[Sample]) -> Vec<String> {
    let mut unavailable = BTreeSet::new();

    let saw_cpu = samples.iter().any(|s| s.cpu_total_percent.is_some());
    let saw_memory = samples.iter().any(|s| s.memory_total_bytes.is_some());
    let saw_process_io = samples.iter().any(|s| {
        s.top_processes_cpu
            .iter()
            .chain(s.top_processes_memory.iter())
            .any(|p| p.io_read_bytes_per_sec.is_some() || p.io_write_bytes_per_sec.is_some())
    });
    let saw_disk_busy = samples.iter().any(|s| s.disk.busy_percent.is_some());
    let saw_disk_latency = samples.iter().any(|s| s.disk.avg_latency_ms.is_some());
    let saw_network = samples.iter().any(|s| {
        s.network.down_bytes_per_sec.is_some() || s.network.up_bytes_per_sec.is_some()
    });

    if !saw_cpu {
        unavailable.insert("overall_cpu_usage".to_string());
    }
    if !saw_memory {
        unavailable.insert("memory_usage".to_string());
    }
    if !saw_process_io {
        unavailable.insert("process_io".to_string());
    }
    if !saw_disk_busy {
        unavailable.insert("disk_busy_percent".to_string());
    }
    if !saw_disk_latency {
        unavailable.insert("disk_latency_ms".to_string());
    }
    if !saw_network {
        unavailable.insert("network_throughput".to_string());
    }

    unavailable.into_iter().collect()
}

fn build_sample(
    system: &sysinfo::System,
    disks: &sysinfo::Disks,
    networks: &sysinfo::Networks,
    top_n: usize,
    interval_secs: f64,
    windows_collector: &mut windows::WindowsCollector,
) -> Sample {
    let timestamp = Utc::now();

    let cpu_total_percent = if system.cpus().is_empty() {
        None
    } else {
        let total = system.cpus().iter().map(|cpu| cpu.cpu_usage()).sum::<f32>();
        Some(total / system.cpus().len() as f32)
    };
    let cpu_per_core_percent = system.cpus().iter().map(|cpu| cpu.cpu_usage()).collect::<Vec<_>>();

    let memory_total = system.total_memory();
    let memory_used = system.used_memory();
    let memory_available = system.available_memory();

    let (memory_total_bytes, memory_used_bytes, memory_available_bytes) = if memory_total == 0 {
        (None, None, None)
    } else {
        (Some(memory_total), Some(memory_used), Some(memory_available))
    };

    let mut processes = Vec::new();
    let mut marker_flags = MarkerFlags::default();
    let cpu_core_count = system.cpus().len();

    for (pid, process) in system.processes() {
        let name = process_name(process);
        let lowered = name.to_ascii_lowercase();

        let is_browser = is_browser_process(&lowered);
        let is_dev_tool = is_dev_tool_process(&lowered);
        let is_marker = is_marker_process(&lowered);

        if lowered == "msmpeng.exe" {
            marker_flags.msmpeng_present = true;
        }
        if lowered == "searchindexer.exe" {
            marker_flags.search_indexer_present = true;
        }
        if lowered == "tiworker.exe" {
            marker_flags.tiworker_present = true;
        }
        if lowered.contains("vmmem") || lowered.contains("wsl") {
            marker_flags.vmmem_present = true;
        }
        if lowered.contains("docker") {
            marker_flags.docker_present = true;
        }
        if is_browser {
            marker_flags.browser_process_count += 1;
        }
        if is_dev_tool {
            marker_flags.dev_tool_process_count += 1;
        }

        let disk_usage = process.disk_usage();
        let read_rate = if interval_secs > 0.0 {
            Some(disk_usage.read_bytes as f64 / interval_secs)
        } else {
            None
        };
        let write_rate = if interval_secs > 0.0 {
            Some(disk_usage.written_bytes as f64 / interval_secs)
        } else {
            None
        };

        processes.push(ProcessSample {
            pid: pid.as_u32(),
            name,
            cpu_percent: Some(normalize_process_cpu(process.cpu_usage(), cpu_core_count)),
            memory_bytes: Some(process.memory()),
            io_read_bytes_per_sec: read_rate,
            io_write_bytes_per_sec: write_rate,
            is_marker,
            is_browser,
            is_dev_tool,
        });
    }

    let process_count = Some(system.processes().len());

    let mut top_processes_cpu = processes.clone();
    top_processes_cpu.sort_by(|a, b| {
        b.cpu_percent
            .unwrap_or(0.0)
            .partial_cmp(&a.cpu_percent.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    top_processes_cpu.truncate(top_n);

    let mut top_processes_memory = processes.clone();
    top_processes_memory.sort_by(|a, b| b.memory_bytes.unwrap_or(0).cmp(&a.memory_bytes.unwrap_or(0)));
    top_processes_memory.truncate(top_n);

    let total_read_rate = processes
        .iter()
        .filter_map(|p| p.io_read_bytes_per_sec)
        .sum::<f64>();
    let total_write_rate = processes
        .iter()
        .filter_map(|p| p.io_write_bytes_per_sec)
        .sum::<f64>();

    let process_io_available = processes.iter().any(|p| {
        p.io_read_bytes_per_sec.is_some() || p.io_write_bytes_per_sec.is_some()
    });

    let (disk_totals, windows_devices) = windows_collector.query_disk_metrics();
    let disk_devices = merge_disk_devices(disks, windows_devices);

    let disk = DiskSample {
        read_bytes_per_sec: disk_totals.read_bytes_per_sec.or(if process_io_available {
            Some(total_read_rate)
        } else {
            None
        }),
        write_bytes_per_sec: disk_totals.write_bytes_per_sec.or(if process_io_available {
            Some(total_write_rate)
        } else {
            None
        }),
        busy_percent: disk_totals.busy_percent,
        avg_latency_ms: disk_totals.avg_latency_ms,
    };
    let network = summarize_network(networks, interval_secs);

    Sample {
        timestamp,
        cpu_total_percent,
        cpu_per_core_percent,
        memory_used_bytes,
        memory_total_bytes,
        memory_available_bytes,
        process_count,
        disk,
        disk_devices,
        network,
        top_processes_cpu,
        top_processes_memory,
        marker_flags,
    }
}

fn summarize_network(networks: &sysinfo::Networks, interval_secs: f64) -> NetworkSample {
    let mut total_down = 0.0_f64;
    let mut total_up = 0.0_f64;
    let mut active = 0usize;

    for (_name, net) in networks {
        let down = if interval_secs > 0.0 {
            net.received() as f64 / interval_secs
        } else {
            net.received() as f64
        };
        let up = if interval_secs > 0.0 {
            net.transmitted() as f64 / interval_secs
        } else {
            net.transmitted() as f64
        };

        total_down += down;
        total_up += up;
        if down > 1024.0 || up > 1024.0 {
            active += 1;
        }
    }

    NetworkSample {
        down_bytes_per_sec: Some(total_down),
        up_bytes_per_sec: Some(total_up),
        interface_count: Some(networks.len()),
        active_interface_count: Some(active),
    }
}

fn merge_disk_devices(
    disks: &sysinfo::Disks,
    windows_devices: Vec<windows::DiskDeviceMetric>,
) -> Vec<DiskDeviceSample> {
    let mut out: HashMap<String, DiskDeviceSample> = HashMap::new();

    for disk in disks.list() {
        let key = mount_to_key(disk.mount_point().to_string_lossy().as_ref());
        let fs = disk.file_system().to_string_lossy().to_string();
        let label = if key != "unknown" {
            key.clone()
        } else {
            disk.name().to_string_lossy().to_string()
        };

        out.insert(
            key.clone(),
            DiskDeviceSample {
                key,
                label,
                total_bytes: Some(disk.total_space()),
                available_bytes: Some(disk.available_space()),
                file_system: Some(fs),
                ..Default::default()
            },
        );
    }

    for device in windows_devices {
        let key = device.key.to_ascii_uppercase();
        let entry = out.entry(key.clone()).or_insert_with(|| DiskDeviceSample {
            key: key.clone(),
            label: if device.label.is_empty() {
                key.clone()
            } else {
                device.label.clone()
            },
            ..Default::default()
        });

        entry.read_bytes_per_sec = device.read_bytes_per_sec;
        entry.write_bytes_per_sec = device.write_bytes_per_sec;
        entry.busy_percent = device.busy_percent;
        entry.avg_latency_ms = device.avg_latency_ms;
        if entry.label == key && !device.label.is_empty() {
            entry.label = device.label;
        }
    }

    let mut list: Vec<DiskDeviceSample> = out.into_values().collect();
    list.sort_by(|a, b| a.key.cmp(&b.key));
    list
}

fn mount_to_key(mount: &str) -> String {
    let m = mount.trim();
    let chars: Vec<char> = m.chars().collect();
    if chars.len() >= 2 && chars[1] == ':' {
        return format!("{}:", chars[0].to_ascii_uppercase());
    }
    "unknown".to_string()
}

fn normalize_process_cpu(raw: f32, cpu_core_count: usize) -> f32 {
    if cpu_core_count == 0 {
        return raw.max(0.0);
    }

    // sysinfo process CPU may be up to (core_count * 100). Normalize to total machine %. 
    (raw / cpu_core_count as f32).clamp(0.0, 100.0)
}

fn process_name(process: &sysinfo::Process) -> String {
    process.name().to_string()
}

fn is_marker_process(name: &str) -> bool {
    matches!(name, "msmpeng.exe" | "searchindexer.exe" | "tiworker.exe")
        || name.contains("vmmem")
        || name.contains("wsl")
        || name.contains("docker")
}

fn is_browser_process(name: &str) -> bool {
    name.contains("chrome") || name.contains("msedge") || name.contains("firefox")
}

fn is_dev_tool_process(name: &str) -> bool {
    name.contains("node")
        || name.contains("npm")
        || name.contains("pnpm")
        || name.contains("yarn")
        || name.contains("tsserver")
        || name.contains("vite")
        || name.contains("webpack")
        || name.contains("docker")
        || name.contains("vmmem")
        || name.contains("wsl")
        || name.contains("cargo")
        || name.contains("rustc")
        || name.contains("devenv")
        || name.contains("code")
}

