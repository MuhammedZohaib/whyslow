use chrono::{TimeZone, Utc};

use whyslow::model::{
    Diagnosis, DiagnosisKind, Evidence, HostInfo, OffenderSummary, Report, RunConfig, Suggestion,
    SystemSummary,
};

#[test]
fn text_report_matches_snapshot_fixture() {
    let report = Report {
        schema_version: "1.0.0".to_string(),
        generated_at: Utc
            .with_ymd_and_hms(2026, 1, 15, 10, 30, 0)
            .single()
            .expect("valid datetime"),
        config: RunConfig {
            sample_window_secs: 20,
            interval_ms: 500,
            top_n: 5,
            json_output: false,
            watch_seconds: None,
            export_path: None,
            verbose: 0,
        },
        host: HostInfo {
            hostname: Some("workstation-01".to_string()),
            os_name: Some("Windows".to_string()),
            os_version: Some("11 Pro".to_string()),
            kernel_version: Some("10.0.22631".to_string()),
            cpu_core_count: 16,
            cpu_logical_core_count: Some(24),
            cpu_physical_core_count: Some(16),
            total_memory_bytes: Some(32 * 1024 * 1024 * 1024),
            uptime_secs: Some(12_345),
        },
        sample_count: 40,
        sample_window_secs: 20,
        summary: SystemSummary {
            avg_cpu_percent: Some(89.2),
            peak_cpu_percent: Some(98.7),
            avg_memory_used_percent: Some(86.1),
            peak_memory_used_percent: Some(92.4),
            avg_process_count: Some(254.0),
            avg_disk_read_bytes_per_sec: Some(45.0 * 1024.0 * 1024.0),
            avg_disk_write_bytes_per_sec: Some(12.0 * 1024.0 * 1024.0),
            avg_disk_busy_percent: Some(87.4),
            peak_disk_busy_percent: Some(98.2),
            avg_disk_latency_ms: Some(24.5),
            peak_disk_latency_ms: Some(80.0),
            avg_network_down_bytes_per_sec: Some(2.5 * 1024.0 * 1024.0),
            peak_network_down_bytes_per_sec: Some(4.0 * 1024.0 * 1024.0),
            avg_network_up_bytes_per_sec: Some(0.8 * 1024.0 * 1024.0),
            peak_network_up_bytes_per_sec: Some(1.2 * 1024.0 * 1024.0),
        },
        diagnoses: vec![Diagnosis {
            kind: DiagnosisKind::CpuSaturation,
            confidence: 0.91,
            duration_seconds: Some(18.0),
            explanation: "CPU stayed high during most of the sampling window, and a small set of processes consumed a large share of compute time.".to_string(),
            evidence: vec![
                Evidence {
                    label: "Average CPU usage".to_string(),
                    detail: "89.2%".to_string(),
                },
                Evidence {
                    label: "Top CPU offenders".to_string(),
                    detail: "Chrome Browser (avg CPU 55.0%, procs 8, hits 40)".to_string(),
                },
            ],
            suggestions: vec![Suggestion {
                action: "Pause or close the top CPU-heavy process first".to_string(),
                rationale:
                    "A single dominant process often provides immediate responsiveness gains."
                        .to_string(),
            }],
            partial_evidence: false,
        }],
        top_offenders: vec![OffenderSummary {
            family: "browser.chrome".to_string(),
            name: "Chrome Browser".to_string(),
            representative_pid: Some(4500),
            process_count: 8,
            member_names: vec!["chrome.exe".to_string(), "chrome_helper.exe".to_string()],
            avg_cpu_percent: 55.0,
            peak_cpu_percent: 89.0,
            avg_memory_bytes: (2.4 * 1024.0 * 1024.0 * 1024.0) as u64,
            peak_memory_bytes: (2.9 * 1024.0 * 1024.0 * 1024.0) as u64,
            avg_io_read_bytes_per_sec: 20.0 * 1024.0 * 1024.0,
            avg_io_write_bytes_per_sec: 6.0 * 1024.0 * 1024.0,
            sample_hits: 40,
            marker_labels: vec!["browser".to_string()],
        }],
        unavailable_metrics: vec![],
    };

    let rendered = whyslow::report::text::render(&report).replace("\r\n", "\n");
    let expected = include_str!("fixtures/report_snapshot.txt").replace("\r\n", "\n");
    let expected = expected.trim_start_matches('\u{feff}');
    assert_eq!(rendered.trim(), expected.trim());
}
