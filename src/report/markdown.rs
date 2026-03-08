use crate::model::Report;

pub fn render(report: &Report) -> String {
    let mut out = String::new();

    out.push_str("# whyslow Report\n\n");
    out.push_str(&format!(
        "- Generated: {}\n",
        report.generated_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str(&format!(
        "- Samples: {} over {}s\n",
        report.sample_count, report.sample_window_secs
    ));
    out.push_str(&format!(
        "- Host: {} | {} {}\n\n",
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

    out.push_str("## System Summary\n\n");
    out.push_str(&format!(
        "- CPU avg/peak: {} / {}\n",
        fmt_opt_percent(report.summary.avg_cpu_percent),
        fmt_opt_percent(report.summary.peak_cpu_percent)
    ));
    out.push_str(&format!(
        "- Memory avg/peak used: {} / {}\n",
        fmt_opt_percent(report.summary.avg_memory_used_percent),
        fmt_opt_percent(report.summary.peak_memory_used_percent)
    ));
    out.push_str(&format!(
        "- Disk read/write: {} / {}\n",
        fmt_opt_mbps(report.summary.avg_disk_read_bytes_per_sec),
        fmt_opt_mbps(report.summary.avg_disk_write_bytes_per_sec)
    ));
    out.push_str(&format!(
        "- Disk busy avg/peak: {} / {}\n",
        fmt_opt_percent(report.summary.avg_disk_busy_percent),
        fmt_opt_percent(report.summary.peak_disk_busy_percent)
    ));
    out.push_str(&format!(
        "- Disk latency avg/peak: {} / {}\n",
        fmt_opt_ms(report.summary.avg_disk_latency_ms),
        fmt_opt_ms(report.summary.peak_disk_latency_ms)
    ));
    out.push_str(&format!(
        "- Network down/up avg: {} / {}\n\n",
        fmt_opt_mbps(report.summary.avg_network_down_bytes_per_sec),
        fmt_opt_mbps(report.summary.avg_network_up_bytes_per_sec)
    ));

    out.push_str("## Likely Bottlenecks\n\n");
    for diagnosis in &report.diagnoses {
        out.push_str(&format!(
            "### {} ({:.0}%, duration {})\n\n{}\n\n",
            diagnosis.kind.as_str(),
            diagnosis.confidence * 100.0,
            diagnosis
                .duration_seconds
                .map(|d| format!("{d:.0}s"))
                .unwrap_or_else(|| "n/a".to_string()),
            diagnosis.explanation
        ));
        out.push_str("Reasons:\n");
        for ev in &diagnosis.evidence {
            out.push_str(&format!("- {}: {}\n", ev.label, ev.detail));
        }
        out.push_str("\nActions:\n");
        for s in &diagnosis.suggestions {
            out.push_str(&format!("- {} ({})\n", s.action, s.rationale));
        }
        out.push('\n');
    }

    out.push_str("## Top Offender Families\n\n");
    for o in &report.top_offenders {
        out.push_str(&format!(
            "- {} [{}]: avg CPU {:.1}%, avg mem {:.2} GB, avg I/O R {:.1} MB/s W {:.1} MB/s\n",
            o.name,
            o.family,
            o.avg_cpu_percent,
            o.avg_memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            o.avg_io_read_bytes_per_sec / (1024.0 * 1024.0),
            o.avg_io_write_bytes_per_sec / (1024.0 * 1024.0)
        ));
    }

    out
}

fn fmt_opt_percent(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1}%"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn fmt_opt_mbps(v: Option<f64>) -> String {
    v.map(|x| format!("{:.1} MB/s", x / (1024.0 * 1024.0)))
        .unwrap_or_else(|| "n/a".to_string())
}

fn fmt_opt_ms(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1} ms"))
        .unwrap_or_else(|| "n/a".to_string())
}
