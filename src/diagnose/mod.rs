mod rules;

use std::collections::{HashMap, HashSet};

use crate::model::{
    clamp_score, AnalysisResult, CollectionWindow, Diagnosis, OffenderSummary, ProcessSample,
    RunConfig, Sample, SystemSummary,
};

/// Run deterministic rules against the collection window.
pub fn analyze(config: &RunConfig, window: &CollectionWindow) -> AnalysisResult {
    let summary = summarize(&window.samples);
    let mut offenders = aggregate_offenders(&window.samples);
    let total_samples = window.samples.len().max(1);
    offenders.sort_by(|a, b| {
        let a_score = offender_score(a, total_samples);
        let b_score = offender_score(b, total_samples);
        b_score
            .partial_cmp(&a_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut diagnoses = Vec::<Diagnosis>::new();
    if let Some(d) = rules::cpu_saturation(window, &summary, &offenders) {
        diagnoses.push(d);
    }
    if let Some(d) = rules::memory_pressure(window, &summary, &offenders) {
        diagnoses.push(d);
    }
    if let Some(d) = rules::disk_contention(window, &summary, &offenders) {
        diagnoses.push(d);
    }
    if let Some(d) = rules::background_scan(window, &summary, &offenders) {
        diagnoses.push(d);
    }
    if let Some(d) = rules::update_activity(window, &summary, &offenders) {
        diagnoses.push(d);
    }
    if let Some(d) = rules::browser_bloat(window, &summary, &offenders) {
        diagnoses.push(d);
    }
    if let Some(d) = rules::dev_tool_storm(window, &summary, &offenders) {
        diagnoses.push(d);
    }

    diagnoses.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    diagnoses.truncate(config.top_n);

    offenders.truncate(config.top_n);

    AnalysisResult {
        summary,
        diagnoses,
        top_offenders: offenders,
    }
}

fn summarize(samples: &[Sample]) -> SystemSummary {
    let cpu_values: Vec<f32> = samples.iter().filter_map(|s| s.cpu_total_percent).collect();
    let memory_used_pct: Vec<f32> = samples
        .iter()
        .filter_map(|s| match (s.memory_used_bytes, s.memory_total_bytes) {
            (Some(used), Some(total)) if total > 0 => Some((used as f32 / total as f32) * 100.0),
            _ => None,
        })
        .collect();
    let process_count: Vec<f32> = samples
        .iter()
        .filter_map(|s| s.process_count.map(|p| p as f32))
        .collect();
    let disk_read: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.disk.read_bytes_per_sec)
        .collect();
    let disk_write: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.disk.write_bytes_per_sec)
        .collect();
    let disk_busy: Vec<f32> = samples.iter().filter_map(|s| s.disk.busy_percent).collect();
    let disk_latency: Vec<f32> = samples
        .iter()
        .filter_map(|s| s.disk.avg_latency_ms)
        .collect();
    let network_down: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.network.down_bytes_per_sec)
        .collect();
    let network_up: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.network.up_bytes_per_sec)
        .collect();

    SystemSummary {
        avg_cpu_percent: mean_f32(&cpu_values),
        peak_cpu_percent: max_f32(&cpu_values),
        avg_memory_used_percent: mean_f32(&memory_used_pct),
        peak_memory_used_percent: max_f32(&memory_used_pct),
        avg_process_count: mean_f32(&process_count),
        avg_disk_read_bytes_per_sec: mean_f64(&disk_read),
        avg_disk_write_bytes_per_sec: mean_f64(&disk_write),
        avg_disk_busy_percent: mean_f32(&disk_busy),
        peak_disk_busy_percent: max_f32(&disk_busy),
        avg_disk_latency_ms: mean_f32(&disk_latency),
        peak_disk_latency_ms: max_f32(&disk_latency),
        avg_network_down_bytes_per_sec: mean_f64(&network_down),
        peak_network_down_bytes_per_sec: max_f64(&network_down),
        avg_network_up_bytes_per_sec: mean_f64(&network_up),
        peak_network_up_bytes_per_sec: max_f64(&network_up),
    }
}

#[derive(Default)]
struct FamilySampleAccumulator {
    family: String,
    name: String,
    cpu_sum: f32,
    cpu_peak: f32,
    memory_sum: u64,
    memory_peak: u64,
    read_sum: f64,
    write_sum: f64,
    representative_pid: Option<u32>,
    representative_cpu: f32,
    members: HashSet<(u32, String)>,
    marker_labels: HashSet<String>,
}

#[derive(Default)]
struct OffenderAccumulator {
    family: String,
    name: String,
    sample_hits: usize,
    cpu_sum: f32,
    cpu_peak: f32,
    memory_sum: u64,
    memory_peak: u64,
    read_sum: f64,
    write_sum: f64,
    representative_pid: Option<u32>,
    representative_cpu: f32,
    member_pids: HashSet<u32>,
    member_names: HashSet<String>,
    marker_labels: HashSet<String>,
}

fn aggregate_offenders(samples: &[Sample]) -> Vec<OffenderSummary> {
    let mut aggregates: HashMap<String, OffenderAccumulator> = HashMap::new();

    for sample in samples {
        let mut deduped_processes: HashMap<(u32, String), ProcessSample> = HashMap::new();
        for proc in sample
            .top_processes_cpu
            .iter()
            .chain(sample.top_processes_memory.iter())
        {
            let key = (proc.pid, proc.name.clone());
            let entry = deduped_processes.entry(key).or_insert_with(|| proc.clone());

            if proc.cpu_percent.unwrap_or(0.0) > entry.cpu_percent.unwrap_or(0.0) {
                entry.cpu_percent = proc.cpu_percent;
            }
            if proc.memory_bytes.unwrap_or(0) > entry.memory_bytes.unwrap_or(0) {
                entry.memory_bytes = proc.memory_bytes;
            }
            if proc.io_read_bytes_per_sec.unwrap_or(0.0)
                > entry.io_read_bytes_per_sec.unwrap_or(0.0)
            {
                entry.io_read_bytes_per_sec = proc.io_read_bytes_per_sec;
            }
            if proc.io_write_bytes_per_sec.unwrap_or(0.0)
                > entry.io_write_bytes_per_sec.unwrap_or(0.0)
            {
                entry.io_write_bytes_per_sec = proc.io_write_bytes_per_sec;
            }
            entry.is_marker |= proc.is_marker;
            entry.is_browser |= proc.is_browser;
            entry.is_dev_tool |= proc.is_dev_tool;
        }

        let mut sample_families: HashMap<String, FamilySampleAccumulator> = HashMap::new();

        for (_key, proc) in deduped_processes {
            let (family, display_name) = process_family(&proc);
            let entry = sample_families.entry(family.clone()).or_default();

            entry.family = family;
            entry.name = display_name;

            let cpu = proc.cpu_percent.unwrap_or(0.0);
            entry.cpu_sum += cpu;
            if cpu > entry.cpu_peak {
                entry.cpu_peak = cpu;
            }

            let mem = proc.memory_bytes.unwrap_or(0);
            entry.memory_sum = entry.memory_sum.saturating_add(mem);
            if mem > entry.memory_peak {
                entry.memory_peak = mem;
            }

            entry.read_sum += proc.io_read_bytes_per_sec.unwrap_or(0.0);
            entry.write_sum += proc.io_write_bytes_per_sec.unwrap_or(0.0);

            if cpu > entry.representative_cpu {
                entry.representative_cpu = cpu;
                entry.representative_pid = Some(proc.pid);
            }

            entry.members.insert((proc.pid, proc.name.clone()));
            if proc.is_marker {
                entry.marker_labels.insert("marker_process".to_string());
            }
            if proc.is_browser {
                entry.marker_labels.insert("browser".to_string());
            }
            if proc.is_dev_tool {
                entry.marker_labels.insert("dev_tool".to_string());
            }
        }

        for (family, fam) in sample_families {
            let acc = aggregates.entry(family.clone()).or_default();

            acc.family = family;
            acc.name = fam.name;
            acc.sample_hits += 1;
            acc.cpu_sum += fam.cpu_sum;
            if fam.cpu_peak > acc.cpu_peak {
                acc.cpu_peak = fam.cpu_peak;
            }
            acc.memory_sum = acc.memory_sum.saturating_add(fam.memory_sum);
            if fam.memory_peak > acc.memory_peak {
                acc.memory_peak = fam.memory_peak;
            }
            acc.read_sum += fam.read_sum;
            acc.write_sum += fam.write_sum;

            if fam.representative_cpu > acc.representative_cpu {
                acc.representative_cpu = fam.representative_cpu;
                acc.representative_pid = fam.representative_pid;
            }

            for (pid, name) in fam.members {
                acc.member_pids.insert(pid);
                acc.member_names.insert(name);
            }
            acc.marker_labels.extend(fam.marker_labels);
        }
    }

    let mut offenders = Vec::new();
    for (_family, acc) in aggregates {
        if acc.sample_hits == 0 {
            continue;
        }

        let mut marker_labels: Vec<String> = acc.marker_labels.into_iter().collect();
        marker_labels.sort();

        let mut member_names: Vec<String> = acc.member_names.into_iter().collect();
        member_names.sort();
        if member_names.len() > 5 {
            member_names.truncate(5);
        }

        offenders.push(OffenderSummary {
            family: acc.family,
            name: acc.name,
            representative_pid: acc.representative_pid,
            process_count: acc.member_pids.len(),
            member_names,
            avg_cpu_percent: acc.cpu_sum / acc.sample_hits as f32,
            peak_cpu_percent: acc.cpu_peak,
            avg_memory_bytes: acc.memory_sum / acc.sample_hits as u64,
            peak_memory_bytes: acc.memory_peak,
            avg_io_read_bytes_per_sec: acc.read_sum / acc.sample_hits as f64,
            avg_io_write_bytes_per_sec: acc.write_sum / acc.sample_hits as f64,
            sample_hits: acc.sample_hits,
            marker_labels,
        });
    }

    offenders
}

fn process_family(process: &ProcessSample) -> (String, String) {
    let lowered = process.name.to_ascii_lowercase();

    if lowered.contains("msmpeng") {
        return ("marker.msmpeng".to_string(), "Windows Defender".to_string());
    }
    if lowered.contains("searchindexer") {
        return (
            "marker.search_indexer".to_string(),
            "Windows Search Indexer".to_string(),
        );
    }
    if lowered.contains("tiworker") {
        return (
            "marker.tiworker".to_string(),
            "Windows Update Installer".to_string(),
        );
    }

    if lowered.contains("chrome") {
        return ("browser.chrome".to_string(), "Chrome Browser".to_string());
    }
    if lowered.contains("msedge") {
        return ("browser.edge".to_string(), "Microsoft Edge".to_string());
    }
    if lowered.contains("firefox") {
        return ("browser.firefox".to_string(), "Firefox Browser".to_string());
    }

    if lowered.contains("docker") {
        return ("dev.docker".to_string(), "Docker Desktop Stack".to_string());
    }
    if lowered.contains("vmmem") || lowered.contains("wsl") {
        return ("dev.wsl".to_string(), "WSL / vmmem".to_string());
    }
    if match_name(
        &lowered,
        &["node", "npm", "pnpm", "yarn", "tsserver", "vite", "webpack"],
    ) {
        return (
            "dev.js_toolchain".to_string(),
            "Node/JS Toolchain".to_string(),
        );
    }

    let base_name = lowered.trim_end_matches(".exe").to_string();
    let display = process.name.trim_end_matches(".exe").to_string();
    (format!("proc.{base_name}"), display)
}

fn offender_score(o: &OffenderSummary, total_samples: usize) -> f64 {
    let presence_ratio = o.sample_hits as f64 / total_samples.max(1) as f64;
    let mem_gb = o.avg_memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let io_mb = (o.avg_io_read_bytes_per_sec + o.avg_io_write_bytes_per_sec) / (1024.0 * 1024.0);

    let mut score = o.avg_cpu_percent as f64
        + (o.peak_cpu_percent as f64 * 0.2)
        + (mem_gb * 8.0)
        + (io_mb * 0.35)
        + (presence_ratio * 30.0)
        + (o.process_count as f64 * 0.6);

    if o.marker_labels.iter().any(|l| l == "browser") {
        score += 3.0;
    }
    if o.marker_labels.iter().any(|l| l == "dev_tool") {
        score += 3.0;
    }
    if o.marker_labels.iter().any(|l| l == "marker_process") {
        score += 2.0;
    }

    score
}

pub(crate) fn ramp(value: f64, low: f64, high: f64) -> f32 {
    if high <= low {
        return 0.0;
    }
    let normalized = (value - low) / (high - low);
    clamp_score(normalized as f32)
}

pub(crate) fn mean_f32(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f32>() / values.len() as f32)
    }
}

pub(crate) fn max_f32(values: &[f32]) -> Option<f32> {
    values
        .iter()
        .copied()
        .reduce(|a, b| if a > b { a } else { b })
}

pub(crate) fn mean_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

pub(crate) fn max_f64(values: &[f64]) -> Option<f64> {
    values
        .iter()
        .copied()
        .reduce(|a, b| if a > b { a } else { b })
}

pub(crate) fn percent(value: f64) -> String {
    format!("{value:.1}%")
}

pub(crate) fn mb_per_sec(value: f64) -> String {
    format!("{:.1} MB/s", value / (1024.0 * 1024.0))
}

pub(crate) fn gb(value: u64) -> String {
    format!("{:.2} GB", value as f64 / (1024.0 * 1024.0 * 1024.0))
}

pub(crate) fn offender_names(offenders: &[OffenderSummary], n: usize) -> String {
    offenders
        .iter()
        .take(n)
        .map(|o| {
            format!(
                "{} (avg CPU {:.1}%, procs {}, hits {})",
                o.name, o.avg_cpu_percent, o.process_count, o.sample_hits
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn match_name(name: &str, needles: &[&str]) -> bool {
    let lowered = name.to_ascii_lowercase();
    needles.iter().any(|n| lowered.contains(n))
}
