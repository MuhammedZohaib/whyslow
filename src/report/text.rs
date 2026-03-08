use std::collections::HashSet;

use crate::model::Report;

pub fn render(report: &Report) -> String {
    let mut out = String::new();

    out.push_str("whyslow report\n");
    out.push_str("=============\n");
    out.push_str(&format!(
        "Generated: {}\n",
        report.generated_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str(&format!(
        "Samples: {} over {}s\n",
        report.sample_count, report.sample_window_secs
    ));
    out.push_str(&format!(
        "Host: {} | {} {}\n\n",
        report
            .host
            .hostname
            .clone()
            .unwrap_or_else(|| "unknown-host".to_string()),
        report
            .host
            .os_name
            .clone()
            .unwrap_or_else(|| "unknown-os".to_string()),
        report
            .host
            .os_version
            .clone()
            .unwrap_or_else(|| "unknown-version".to_string())
    ));

    out.push_str("1) System Summary\n");
    out.push_str(&format!(
        "- CPU avg/peak: {} / {}\n",
        fmt_opt_percent_f32(report.summary.avg_cpu_percent),
        fmt_opt_percent_f32(report.summary.peak_cpu_percent)
    ));
    out.push_str(&format!(
        "- Memory avg/peak used: {} / {}\n",
        fmt_opt_percent_f32(report.summary.avg_memory_used_percent),
        fmt_opt_percent_f32(report.summary.peak_memory_used_percent)
    ));
    out.push_str(&format!(
        "- Avg process count: {}\n",
        report
            .summary
            .avg_process_count
            .map(|v| format!("{v:.1}"))
            .unwrap_or_else(|| "n/a".to_string())
    ));
    out.push_str(&format!(
        "- Disk read/write: {} / {}\n",
        fmt_opt_mbps(report.summary.avg_disk_read_bytes_per_sec),
        fmt_opt_mbps(report.summary.avg_disk_write_bytes_per_sec)
    ));
    out.push_str(&format!(
        "- Disk busy avg/peak: {} / {}\n",
        fmt_opt_percent_f32(report.summary.avg_disk_busy_percent),
        fmt_opt_percent_f32(report.summary.peak_disk_busy_percent)
    ));
    out.push_str(&format!(
        "- Disk latency avg/peak: {} / {}\n",
        fmt_opt_ms(report.summary.avg_disk_latency_ms),
        fmt_opt_ms(report.summary.peak_disk_latency_ms)
    ));
    out.push_str(&format!(
        "- Network down/up avg: {} / {}\n",
        fmt_opt_mbps(report.summary.avg_network_down_bytes_per_sec),
        fmt_opt_mbps(report.summary.avg_network_up_bytes_per_sec)
    ));
    if !report.unavailable_metrics.is_empty() {
        out.push_str(&format!(
            "- Partial metrics: {}\n",
            report.unavailable_metrics.join(", ")
        ));
    }
    out.push('\n');

    out.push_str("2) Likely Bottlenecks (Ranked)\n");
    if report.diagnoses.is_empty() {
        out.push_str("- No strong bottleneck matched current rules. Try a longer sample with `--duration 40`.\n\n");
    } else {
        for (idx, diagnosis) in report.diagnoses.iter().enumerate() {
            out.push_str(&format!(
                "{}. {} (confidence {:.0}%, duration {})\n",
                idx + 1,
                diagnosis.kind.as_str(),
                diagnosis.confidence * 100.0,
                diagnosis
                    .duration_seconds
                    .map(|d| format!("{d:.0}s"))
                    .unwrap_or_else(|| "n/a".to_string())
            ));
            out.push_str(&format!("   Explanation: {}\n", diagnosis.explanation));
            for evidence in &diagnosis.evidence {
                out.push_str(&format!("   Evidence: {} -> {}\n", evidence.label, evidence.detail));
            }
            if diagnosis.partial_evidence {
                out.push_str("   Evidence quality: partial (some metrics unavailable)\n");
            }
        }
        out.push('\n');
    }

    out.push_str("3) Top Offenders (Grouped Families)\n");
    if report.top_offenders.is_empty() {
        out.push_str("- n/a\n\n");
    } else {
        for offender in &report.top_offenders {
            let pid = offender
                .representative_pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "n/a".to_string());
            let members = if offender.member_names.is_empty() {
                "n/a".to_string()
            } else {
                offender.member_names.join(", ")
            };

            out.push_str(&format!(
                "- {} [{}] rep PID {}: avg CPU {:.1}%, peak CPU {:.1}%, avg mem {}, avg I/O R {} W {}, members {}\n",
                offender.name,
                offender.family,
                pid,
                offender.avg_cpu_percent,
                offender.peak_cpu_percent,
                fmt_bytes(offender.avg_memory_bytes),
                fmt_mbps(offender.avg_io_read_bytes_per_sec),
                fmt_mbps(offender.avg_io_write_bytes_per_sec),
                members
            ));
        }
        out.push('\n');
    }

    out.push_str("4) Suggested Actions\n");
    let mut seen = HashSet::new();
    let mut action_lines = Vec::new();
    for diagnosis in &report.diagnoses {
        for suggestion in &diagnosis.suggestions {
            if seen.insert(suggestion.action.clone()) {
                action_lines.push(format!(
                    "- {} (why: {})",
                    suggestion.action, suggestion.rationale
                ));
            }
        }
    }

    if action_lines.is_empty() {
        out.push_str("- No action recommendations available for this run.\n");
    } else {
        for line in action_lines {
            out.push_str(&line);
            out.push('\n');
        }
    }

    out
}

fn fmt_bytes(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn fmt_mbps(bytes_per_sec: f64) -> String {
    format!("{:.1} MB/s", bytes_per_sec / (1024.0 * 1024.0))
}

fn fmt_opt_mbps(bytes_per_sec: Option<f64>) -> String {
    bytes_per_sec
        .map(fmt_mbps)
        .unwrap_or_else(|| "n/a".to_string())
}

fn fmt_opt_percent_f32(v: Option<f32>) -> String {
    v.map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn fmt_opt_ms(v: Option<f32>) -> String {
    v.map(|value| format!("{value:.1} ms"))
        .unwrap_or_else(|| "n/a".to_string())
}
