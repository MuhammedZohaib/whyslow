use crate::model::{
    clamp_score, CollectionWindow, Diagnosis, DiagnosisKind, Evidence, OffenderSummary, Suggestion,
    SystemSummary,
};

use super::{gb, mb_per_sec, offender_names, percent, ramp};

pub(super) fn cpu_saturation(
    window: &CollectionWindow,
    summary: &SystemSummary,
    offenders: &[OffenderSummary],
) -> Option<Diagnosis> {
    let avg_cpu = summary.avg_cpu_percent?;
    let peak_cpu = summary.peak_cpu_percent.unwrap_or(avg_cpu);

    let top_cpu = offenders
        .iter()
        .take(3)
        .map(|o| o.avg_cpu_percent)
        .sum::<f32>();
    let sustained = ramp(avg_cpu as f64, 60.0, 95.0);
    let peak = ramp(peak_cpu as f64, 75.0, 100.0);
    let dominance = ramp(top_cpu as f64, 30.0, 180.0);

    let confidence = clamp_score(0.50 * sustained + 0.25 * peak + 0.25 * dominance);
    if confidence < 0.25 {
        return None;
    }

    let high_cpu_secs = sustained_seconds(window, |s| s.cpu_total_percent.unwrap_or(0.0) >= 85.0);
    let duration_label = duration_class(high_cpu_secs);

    let explanation = if high_cpu_secs < 5.0 {
        "CPU spike detected, but duration appears short."
    } else {
        "CPU stayed high during most of the sampling window, and a small set of processes consumed a large share of compute time."
    };

    Some(Diagnosis {
        kind: DiagnosisKind::CpuSaturation,
        confidence,
        duration_seconds: Some(high_cpu_secs as f32),
        explanation: explanation.to_string(),
        evidence: vec![
            Evidence {
                label: "Average CPU usage".to_string(),
                detail: percent(avg_cpu as f64),
            },
            Evidence {
                label: "Peak CPU usage".to_string(),
                detail: percent(peak_cpu as f64),
            },
            Evidence {
                label: "Top CPU offenders".to_string(),
                detail: offender_names(offenders, 3),
            },
            Evidence {
                label: "Duration profile".to_string(),
                detail: format!("{duration_label} ({high_cpu_secs:.1}s above 85%)"),
            },
        ],
        suggestions: vec![
            Suggestion {
                action: "Pause or close the top CPU-heavy process first".to_string(),
                rationale:
                    "A single dominant process often provides immediate responsiveness gains."
                        .to_string(),
            },
            Suggestion {
                action: "Check whether antivirus scans, indexing, or build tasks are running"
                    .to_string(),
                rationale: "These workloads can legitimately spike CPU and are usually deferrable."
                    .to_string(),
            },
        ],
        partial_evidence: false,
    })
}

pub(super) fn memory_pressure(
    window: &CollectionWindow,
    summary: &SystemSummary,
    offenders: &[OffenderSummary],
) -> Option<Diagnosis> {
    let avg_used = summary.avg_memory_used_percent?;
    let peak_used = summary.peak_memory_used_percent.unwrap_or(avg_used);

    let min_available_ratio = window
        .samples
        .iter()
        .filter_map(|s| match (s.memory_available_bytes, s.memory_total_bytes) {
            (Some(avail), Some(total)) if total > 0 => Some(avail as f64 / total as f64),
            _ => None,
        })
        .reduce(|a, b| a.min(b))?;

    let top_mem_process = offenders
        .iter()
        .max_by_key(|o| o.avg_memory_bytes)
        .map(|o| (o.name.clone(), o.avg_memory_bytes));

    let used_score = ramp(avg_used as f64, 70.0, 95.0);
    let avail_score = ramp((1.0 - min_available_ratio) * 100.0, 70.0, 97.0);
    let peak_score = ramp(peak_used as f64, 80.0, 98.0);
    let confidence = clamp_score(0.45 * used_score + 0.35 * avail_score + 0.20 * peak_score);
    if confidence < 0.25 {
        return None;
    }

    let pressure_secs = sustained_seconds(window, |s| {
        match (s.memory_used_bytes, s.memory_total_bytes) {
            (Some(used), Some(total)) if total > 0 => (used as f64 * 100.0 / total as f64) >= 85.0,
            _ => false,
        }
    });

    let mut evidence = vec![
        Evidence {
            label: "Average memory used".to_string(),
            detail: percent(avg_used as f64),
        },
        Evidence {
            label: "Peak memory used".to_string(),
            detail: percent(peak_used as f64),
        },
        Evidence {
            label: "Lowest available memory ratio".to_string(),
            detail: percent((min_available_ratio * 100.0).max(0.0)),
        },
        Evidence {
            label: "Duration profile".to_string(),
            detail: format!(
                "{} ({pressure_secs:.1}s above 85%)",
                duration_class(pressure_secs)
            ),
        },
    ];

    if let Some((name, bytes)) = top_mem_process {
        evidence.push(Evidence {
            label: "Largest memory offender".to_string(),
            detail: format!("{name} using {}", gb(bytes)),
        });
    }

    Some(Diagnosis {
        kind: DiagnosisKind::MemoryPressure,
        confidence,
        duration_seconds: Some(pressure_secs as f32),
        explanation: "Available memory dropped low while overall memory use remained high, increasing paging risk and UI stalls.".to_string(),
        evidence,
        suggestions: vec![
            Suggestion {
                action: "Close the highest-memory process or heavy browser tabs".to_string(),
                rationale: "Freeing memory reduces paging and often improves responsiveness quickly.".to_string(),
            },
            Suggestion {
                action: "Restart long-lived tools with growing memory usage".to_string(),
                rationale: "Developer tools and browsers can accumulate memory over time.".to_string(),
            },
        ],
        partial_evidence: false,
    })
}

pub(super) fn disk_contention(
    window: &CollectionWindow,
    summary: &SystemSummary,
    offenders: &[OffenderSummary],
) -> Option<Diagnosis> {
    let avg_read = summary.avg_disk_read_bytes_per_sec?;
    let avg_write = summary.avg_disk_write_bytes_per_sec.unwrap_or(0.0);
    let total_io = avg_read + avg_write;

    let busy_values: Vec<f32> = window
        .samples
        .iter()
        .filter_map(|s| s.disk.busy_percent)
        .collect();
    let latency_values: Vec<f32> = window
        .samples
        .iter()
        .filter_map(|s| s.disk.avg_latency_ms)
        .collect();

    let avg_busy = if busy_values.is_empty() {
        None
    } else {
        Some(busy_values.iter().sum::<f32>() / busy_values.len() as f32)
    };
    let avg_latency = if latency_values.is_empty() {
        None
    } else {
        Some(latency_values.iter().sum::<f32>() / latency_values.len() as f32)
    };

    let throughput_score = ramp(total_io / (1024.0 * 1024.0), 20.0, 220.0);
    let busy_score = avg_busy
        .map(|v| ramp(v as f64, 55.0, 97.0))
        .unwrap_or(throughput_score * 0.5);
    let latency_score = avg_latency
        .map(|v| ramp(v as f64, 10.0, 80.0))
        .unwrap_or(throughput_score * 0.4);

    let confidence = clamp_score(0.5 * throughput_score + 0.3 * busy_score + 0.2 * latency_score);
    if confidence < 0.20 {
        return None;
    }

    let contention_secs = sustained_seconds(window, |s| {
        s.disk.busy_percent.unwrap_or(0.0) >= 80.0 || s.disk.avg_latency_ms.unwrap_or(0.0) >= 20.0
    });

    let mut evidence = vec![
        Evidence {
            label: "Average disk throughput (read+write)".to_string(),
            detail: mb_per_sec(total_io),
        },
        Evidence {
            label: "Top I/O offenders".to_string(),
            detail: offenders
                .iter()
                .take(3)
                .map(|o| {
                    format!(
                        "{} (R {}, W {})",
                        o.name,
                        mb_per_sec(o.avg_io_read_bytes_per_sec),
                        mb_per_sec(o.avg_io_write_bytes_per_sec)
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        },
        Evidence {
            label: "Duration profile".to_string(),
            detail: format!(
                "{} ({contention_secs:.1}s busy/latency pressure)",
                duration_class(contention_secs)
            ),
        },
    ];

    if let Some(busy) = avg_busy {
        evidence.push(Evidence {
            label: "Average disk busy".to_string(),
            detail: percent(busy as f64),
        });
    }
    if let Some(latency) = avg_latency {
        evidence.push(Evidence {
            label: "Average disk latency".to_string(),
            detail: format!("{latency:.1} ms"),
        });
    }

    let partial = avg_busy.is_none() || avg_latency.is_none();
    if partial {
        evidence.push(Evidence {
            label: "Partial evidence notice".to_string(),
            detail: "Disk busy percent/latency counters were unavailable; score used throughput and process I/O only.".to_string(),
        });
    }

    Some(Diagnosis {
        kind: DiagnosisKind::DiskContention,
        confidence,
        duration_seconds: Some(contention_secs as f32),
        explanation: "Sustained disk throughput suggests storage contention, which can stall app launches and file operations.".to_string(),
        evidence,
        suggestions: vec![
            Suggestion {
                action: "Pause large downloads, sync jobs, or indexing temporarily".to_string(),
                rationale: "Reducing concurrent I/O lowers queue depth and improves responsiveness.".to_string(),
            },
            Suggestion {
                action: "Exclude build/cache folders from real-time scanning where safe".to_string(),
                rationale: "Frequent writes from toolchains can trigger heavy scanning overhead.".to_string(),
            },
        ],
        partial_evidence: partial,
    })
}

pub(super) fn background_scan(
    window: &CollectionWindow,
    _summary: &SystemSummary,
    offenders: &[OffenderSummary],
) -> Option<Diagnosis> {
    let matching: Vec<&OffenderSummary> = offenders
        .iter()
        .filter(|o| o.family == "marker.msmpeng" || o.family == "marker.search_indexer")
        .collect();

    let presence_ratio = window
        .samples
        .iter()
        .filter(|s| s.marker_flags.msmpeng_present || s.marker_flags.search_indexer_present)
        .count() as f64
        / window.samples.len().max(1) as f64;

    let cpu = matching
        .iter()
        .map(|o| o.avg_cpu_percent as f64)
        .sum::<f64>();
    let io = matching
        .iter()
        .map(|o| o.avg_io_read_bytes_per_sec + o.avg_io_write_bytes_per_sec)
        .sum::<f64>();

    let confidence = clamp_score(
        0.45 * ramp(presence_ratio * 100.0, 20.0, 95.0)
            + 0.35 * ramp(cpu, 2.0, 45.0)
            + 0.20 * ramp(io / (1024.0 * 1024.0), 1.0, 120.0),
    );

    if confidence < 0.25 {
        return None;
    }

    let active_secs = presence_ratio * window.sample_window_seconds();

    Some(Diagnosis {
        kind: DiagnosisKind::BackgroundScan,
        confidence,
        duration_seconds: Some(active_secs as f32),
        explanation: "Windows background scanning/indexing appears active and is consuming measurable CPU or disk I/O.".to_string(),
        evidence: vec![
            Evidence {
                label: "Marker process presence".to_string(),
                detail: percent((presence_ratio * 100.0).max(0.0)),
            },
            Evidence {
                label: "Scanner/indexer CPU".to_string(),
                detail: percent(cpu),
            },
            Evidence {
                label: "Scanner/indexer I/O".to_string(),
                detail: mb_per_sec(io),
            },
            Evidence {
                label: "Duration profile".to_string(),
                detail: format!("{} ({active_secs:.1}s active)", duration_class(active_secs)),
            },
        ],
        suggestions: vec![
            Suggestion {
                action: "Allow scan/indexing to finish if this is temporary".to_string(),
                rationale: "These tasks often settle after an initial burst.".to_string(),
            },
            Suggestion {
                action: "Schedule heavy scans outside active work hours".to_string(),
                rationale: "Avoids scan contention during interactive use.".to_string(),
            },
        ],
        partial_evidence: false,
    })
}

pub(super) fn update_activity(
    window: &CollectionWindow,
    _summary: &SystemSummary,
    offenders: &[OffenderSummary],
) -> Option<Diagnosis> {
    let matching: Vec<&OffenderSummary> = offenders
        .iter()
        .filter(|o| o.family == "marker.tiworker")
        .collect();

    let presence_ratio = window
        .samples
        .iter()
        .filter(|s| s.marker_flags.tiworker_present)
        .count() as f64
        / window.samples.len().max(1) as f64;

    let cpu = matching
        .iter()
        .map(|o| o.avg_cpu_percent as f64)
        .sum::<f64>();
    let io = matching
        .iter()
        .map(|o| o.avg_io_read_bytes_per_sec + o.avg_io_write_bytes_per_sec)
        .sum::<f64>();

    let confidence = clamp_score(
        0.40 * ramp(presence_ratio * 100.0, 10.0, 95.0)
            + 0.35 * ramp(cpu, 1.0, 35.0)
            + 0.25 * ramp(io / (1024.0 * 1024.0), 1.0, 90.0),
    );

    if confidence < 0.25 {
        return None;
    }

    let active_secs = presence_ratio * window.sample_window_seconds();

    Some(Diagnosis {
        kind: DiagnosisKind::UpdateActivity,
        confidence,
        duration_seconds: Some(active_secs as f32),
        explanation: "Windows update servicing processes are active and likely competing for CPU or disk resources.".to_string(),
        evidence: vec![
            Evidence {
                label: "TiWorker presence".to_string(),
                detail: percent((presence_ratio * 100.0).max(0.0)),
            },
            Evidence {
                label: "Update-process CPU".to_string(),
                detail: percent(cpu),
            },
            Evidence {
                label: "Update-process I/O".to_string(),
                detail: mb_per_sec(io),
            },
            Evidence {
                label: "Duration profile".to_string(),
                detail: format!("{} ({active_secs:.1}s active)", duration_class(active_secs)),
            },
        ],
        suggestions: vec![
            Suggestion {
                action: "Keep the machine on power and let update servicing complete".to_string(),
                rationale: "Interrupting update servicing can prolong overall slowdown.".to_string(),
            },
            Suggestion {
                action: "Restart after updates complete".to_string(),
                rationale: "Pending servicing and file replacement frequently clears after reboot.".to_string(),
            },
        ],
        partial_evidence: false,
    })
}

pub(super) fn browser_bloat(
    window: &CollectionWindow,
    _summary: &SystemSummary,
    offenders: &[OffenderSummary],
) -> Option<Diagnosis> {
    let avg_browser_count = window
        .samples
        .iter()
        .map(|s| s.marker_flags.browser_process_count as f64)
        .sum::<f64>()
        / window.samples.len().max(1) as f64;

    let browser_offenders: Vec<&OffenderSummary> = offenders
        .iter()
        .filter(|o| o.family.starts_with("browser."))
        .collect();

    let browser_memory = browser_offenders
        .iter()
        .map(|o| o.avg_memory_bytes)
        .sum::<u64>();
    let browser_cpu = browser_offenders
        .iter()
        .map(|o| o.avg_cpu_percent as f64)
        .sum::<f64>();

    let confidence = clamp_score(
        0.45 * ramp(avg_browser_count, 8.0, 50.0)
            + 0.35
                * ramp(
                    browser_memory as f64 / (1024.0 * 1024.0 * 1024.0),
                    1.5,
                    10.0,
                )
            + 0.20 * ramp(browser_cpu, 5.0, 90.0),
    );

    if confidence < 0.25 {
        return None;
    }

    let browser_heavy_secs =
        sustained_seconds(window, |s| s.marker_flags.browser_process_count >= 8);

    Some(Diagnosis {
        kind: DiagnosisKind::BrowserBloat,
        confidence,
        duration_seconds: Some(browser_heavy_secs as f32),
        explanation: "Browser process count and memory footprint are high enough to impact interactive performance.".to_string(),
        evidence: vec![
            Evidence {
                label: "Average browser process count".to_string(),
                detail: format!("{avg_browser_count:.1}"),
            },
            Evidence {
                label: "Aggregate browser memory".to_string(),
                detail: gb(browser_memory),
            },
            Evidence {
                label: "Aggregate browser CPU".to_string(),
                detail: percent(browser_cpu),
            },
            Evidence {
                label: "Duration profile".to_string(),
                detail: format!(
                    "{} ({browser_heavy_secs:.1}s with >=8 browser processes)",
                    duration_class(browser_heavy_secs)
                ),
            },
        ],
        suggestions: vec![
            Suggestion {
                action: "Close idle tabs/windows and disable heavy extensions".to_string(),
                rationale: "Large tab trees and extensions are frequent browser slowdown sources.".to_string(),
            },
            Suggestion {
                action: "Use browser task manager to kill high-memory tabs".to_string(),
                rationale: "Targeted tab cleanup avoids restarting all active sessions.".to_string(),
            },
        ],
        partial_evidence: false,
    })
}

pub(super) fn dev_tool_storm(
    window: &CollectionWindow,
    _summary: &SystemSummary,
    offenders: &[OffenderSummary],
) -> Option<Diagnosis> {
    let avg_dev_count = window
        .samples
        .iter()
        .map(|s| s.marker_flags.dev_tool_process_count as f64)
        .sum::<f64>()
        / window.samples.len().max(1) as f64;

    let marker_ratio = window
        .samples
        .iter()
        .filter(|s| s.marker_flags.vmmem_present || s.marker_flags.docker_present)
        .count() as f64
        / window.samples.len().max(1) as f64;

    let dev_offenders: Vec<&OffenderSummary> = offenders
        .iter()
        .filter(|o| o.family.starts_with("dev."))
        .collect();

    let dev_cpu = dev_offenders
        .iter()
        .map(|o| o.avg_cpu_percent as f64)
        .sum::<f64>();

    let dev_io = dev_offenders
        .iter()
        .map(|o| o.avg_io_read_bytes_per_sec + o.avg_io_write_bytes_per_sec)
        .sum::<f64>();

    let confidence = clamp_score(
        0.40 * ramp(avg_dev_count, 4.0, 28.0)
            + 0.35 * ramp(dev_cpu, 8.0, 120.0)
            + 0.25
                * ramp(
                    (marker_ratio * 100.0) + (dev_io / (1024.0 * 1024.0)),
                    15.0,
                    140.0,
                ),
    );

    if confidence < 0.25 {
        return None;
    }

    let dev_secs = sustained_seconds(window, |s| s.marker_flags.dev_tool_process_count >= 4);
    let detected = format!(
        "{} node/tool processes avg, markers present {:.0}%",
        avg_dev_count,
        marker_ratio * 100.0
    );

    Some(Diagnosis {
        kind: DiagnosisKind::DevToolStorm,
        confidence,
        duration_seconds: Some(dev_secs as f32),
        explanation: "Developer stack processes (build tools, containers, WSL) are collectively consuming significant resources.".to_string(),
        evidence: vec![
            Evidence {
                label: "Average dev-tool process count".to_string(),
                detail: format!("{avg_dev_count:.1}"),
            },
            Evidence {
                label: "Aggregate dev-tool CPU".to_string(),
                detail: percent(dev_cpu),
            },
            Evidence {
                label: "Aggregate dev-tool I/O".to_string(),
                detail: mb_per_sec(dev_io),
            },
            Evidence {
                label: "Detected".to_string(),
                detail: detected,
            },
            Evidence {
                label: "Duration profile".to_string(),
                detail: format!("{} ({dev_secs:.1}s active)", duration_class(dev_secs)),
            },
        ],
        suggestions: vec![
            Suggestion {
                action: "Throttle parallel builds/watchers and stop inactive containers".to_string(),
                rationale: "Reducing concurrent dev workloads can quickly free CPU and memory.".to_string(),
            },
            Suggestion {
                action: "Restart WSL/Docker if resource usage remains abnormally high".to_string(),
                rationale: "Long-running sessions can accumulate memory and background I/O.".to_string(),
            },
        ],
        partial_evidence: false,
    })
}

fn sustained_seconds(
    window: &CollectionWindow,
    mut predicate: impl FnMut(&crate::model::Sample) -> bool,
) -> f64 {
    let samples = window.samples.iter().filter(|s| predicate(s)).count() as f64;
    samples * (window.interval_ms as f64 / 1000.0)
}

fn duration_class(seconds: f64) -> &'static str {
    if seconds < 5.0 {
        "Transient (<5s)"
    } else if seconds <= 60.0 {
        "Sustained (5-60s)"
    } else {
        "Persistent (>60s)"
    }
}

trait WindowSeconds {
    fn sample_window_seconds(&self) -> f64;
}

impl WindowSeconds for CollectionWindow {
    fn sample_window_seconds(&self) -> f64 {
        ((self.samples.len() as u64 * self.interval_ms) as f64) / 1000.0
    }
}
