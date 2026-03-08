use chrono::{Duration, Utc};

use whyslow::diagnose;
use whyslow::model::{
    CollectionWindow, HostInfo, MarkerFlags, ProcessSample, RunConfig, Sample,
};

fn config() -> RunConfig {
    RunConfig {
        sample_window_secs: 20,
        interval_ms: 500,
        top_n: 5,
        json_output: false,
        watch_seconds: None,
        export_path: None,
        verbose: 0,
    }
}

fn proc(
    pid: u32,
    name: &str,
    cpu: f32,
    mem_gb: f64,
    read_mb: f64,
    write_mb: f64,
    marker: bool,
) -> ProcessSample {
    ProcessSample {
        pid,
        name: name.to_string(),
        cpu_percent: Some(cpu),
        memory_bytes: Some((mem_gb * 1024.0 * 1024.0 * 1024.0) as u64),
        io_read_bytes_per_sec: Some(read_mb * 1024.0 * 1024.0),
        io_write_bytes_per_sec: Some(write_mb * 1024.0 * 1024.0),
        is_marker: marker,
        is_browser: name.to_ascii_lowercase().contains("chrome"),
        is_dev_tool: false,
    }
}

fn sample(
    cpu: f32,
    mem_used_gb: f64,
    mem_total_gb: f64,
    mem_available_gb: f64,
    markers: MarkerFlags,
    top_cpu: Vec<ProcessSample>,
    top_mem: Vec<ProcessSample>,
) -> Sample {
    Sample {
        timestamp: Utc::now(),
        cpu_total_percent: Some(cpu),
        cpu_per_core_percent: vec![],
        memory_used_bytes: Some((mem_used_gb * 1024.0 * 1024.0 * 1024.0) as u64),
        memory_total_bytes: Some((mem_total_gb * 1024.0 * 1024.0 * 1024.0) as u64),
        memory_available_bytes: Some((mem_available_gb * 1024.0 * 1024.0 * 1024.0) as u64),
        process_count: Some(220),
        disk: whyslow::model::DiskSample {
            read_bytes_per_sec: Some(20.0 * 1024.0 * 1024.0),
            write_bytes_per_sec: Some(8.0 * 1024.0 * 1024.0),
            busy_percent: Some(75.0),
            avg_latency_ms: Some(22.0),
        },
        disk_devices: vec![],
        network: whyslow::model::NetworkSample::default(),
        top_processes_cpu: top_cpu,
        top_processes_memory: top_mem,
        marker_flags: markers,
    }
}

fn window(samples: Vec<Sample>) -> CollectionWindow {
    CollectionWindow {
        started_at: Utc::now(),
        ended_at: Utc::now() + Duration::seconds(20),
        interval_ms: 500,
        host: HostInfo::default(),
        samples,
        unavailable_metrics: vec![],
    }
}

#[test]
fn detects_cpu_saturation() {
    let mut samples = Vec::new();
    for _ in 0..12 {
        samples.push(sample(
            94.0,
            7.0,
            16.0,
            9.0,
            MarkerFlags::default(),
            vec![proc(1000, "renderer.exe", 72.0, 1.1, 2.0, 1.0, false)],
            vec![proc(1000, "renderer.exe", 72.0, 1.1, 2.0, 1.0, false)],
        ));
    }

    let analysis = diagnose::analyze(&config(), &window(samples));

    let cpu = analysis
        .diagnoses
        .iter()
        .find(|d| d.kind == whyslow::model::DiagnosisKind::CpuSaturation)
        .expect("cpu_saturation should be present");

    assert!(cpu.confidence >= 0.60, "unexpected confidence: {}", cpu.confidence);
}

#[test]
fn detects_memory_pressure() {
    let mut samples = Vec::new();
    for _ in 0..10 {
        samples.push(sample(
            48.0,
            15.2,
            16.0,
            0.8,
            MarkerFlags {
                browser_process_count: 18,
                ..MarkerFlags::default()
            },
            vec![proc(1001, "chrome.exe", 18.0, 4.2, 1.0, 0.2, false)],
            vec![proc(1001, "chrome.exe", 18.0, 4.2, 1.0, 0.2, false)],
        ));
    }

    let analysis = diagnose::analyze(&config(), &window(samples));

    let mem = analysis
        .diagnoses
        .iter()
        .find(|d| d.kind == whyslow::model::DiagnosisKind::MemoryPressure)
        .expect("memory_pressure should be present");

    assert!(mem.confidence >= 0.60, "unexpected confidence: {}", mem.confidence);
}

#[test]
fn detects_background_scan() {
    let mut samples = Vec::new();
    for _ in 0..10 {
        samples.push(sample(
            62.0,
            8.0,
            16.0,
            8.0,
            MarkerFlags {
                msmpeng_present: true,
                search_indexer_present: true,
                ..MarkerFlags::default()
            },
            vec![proc(1002, "MsMpEng.exe", 19.0, 0.7, 32.0, 12.0, true)],
            vec![proc(1002, "MsMpEng.exe", 19.0, 0.7, 32.0, 12.0, true)],
        ));
    }

    let analysis = diagnose::analyze(&config(), &window(samples));

    let bg = analysis
        .diagnoses
        .iter()
        .find(|d| d.kind == whyslow::model::DiagnosisKind::BackgroundScan)
        .expect("background_scan should be present");

    assert!(bg.confidence >= 0.50, "unexpected confidence: {}", bg.confidence);
}

#[test]
fn groups_browser_family_into_single_offender() {
    let mut samples = Vec::new();
    for _ in 0..8 {
        samples.push(sample(
            82.0,
            10.0,
            16.0,
            6.0,
            MarkerFlags {
                browser_process_count: 14,
                ..MarkerFlags::default()
            },
            vec![
                proc(4001, "chrome.exe", 28.0, 1.2, 4.0, 1.5, false),
                proc(4002, "chrome.exe", 21.0, 0.9, 3.0, 1.0, false),
                proc(4010, "msedge.exe", 9.0, 0.8, 1.0, 0.6, false),
            ],
            vec![
                proc(4001, "chrome.exe", 28.0, 1.2, 4.0, 1.5, false),
                proc(4002, "chrome.exe", 21.0, 0.9, 3.0, 1.0, false),
            ],
        ));
    }

    let analysis = diagnose::analyze(&config(), &window(samples));

    let chrome = analysis
        .top_offenders
        .iter()
        .find(|o| o.family == "browser.chrome")
        .expect("chrome family should be present");

    assert!(chrome.process_count >= 2);
    assert!(chrome.avg_cpu_percent > 40.0);
}

#[test]
fn classifies_short_cpu_spike_as_transient() {
    let mut samples = Vec::new();
    for _ in 0..3 {
        samples.push(sample(
            95.0,
            7.0,
            16.0,
            9.0,
            MarkerFlags::default(),
            vec![proc(9000, "build.exe", 70.0, 0.8, 1.0, 0.5, false)],
            vec![proc(9000, "build.exe", 70.0, 0.8, 1.0, 0.5, false)],
        ));
    }

    let analysis = diagnose::analyze(&config(), &window(samples));
    let cpu = analysis
        .diagnoses
        .iter()
        .find(|d| d.kind == whyslow::model::DiagnosisKind::CpuSaturation)
        .expect("cpu_saturation should be present");

    assert!(cpu.explanation.contains("spike"));
    let duration = cpu
        .evidence
        .iter()
        .find(|e| e.label == "Duration profile")
        .expect("duration evidence should exist");
    assert!(duration.detail.contains("Transient"));
}
