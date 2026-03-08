use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Runtime configuration for a single `whyslow` run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    /// Sampling window size in seconds.
    pub sample_window_secs: u64,
    /// Delay between samples in milliseconds.
    pub interval_ms: u64,
    /// Maximum diagnoses/offenders to print.
    pub top_n: usize,
    /// Output machine-readable JSON when true.
    pub json_output: bool,
    /// Optional watch cadence in seconds.
    pub watch_seconds: Option<u64>,
    /// Optional export file path (json or markdown extension).
    pub export_path: Option<String>,
    /// Verbosity level from CLI flags.
    pub verbose: u8,
}

/// Basic host metadata captured with each report.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostInfo {
    pub hostname: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub kernel_version: Option<String>,
    /// Backward-compatible alias for logical core count.
    pub cpu_core_count: usize,
    pub cpu_logical_core_count: Option<usize>,
    pub cpu_physical_core_count: Option<usize>,
    pub total_memory_bytes: Option<u64>,
    pub uptime_secs: Option<u64>,
}

/// Raw samples for one collection window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionWindow {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub interval_ms: u64,
    pub host: HostInfo,
    pub samples: Vec<Sample>,
    pub unavailable_metrics: Vec<String>,
}

/// One point-in-time snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub timestamp: DateTime<Utc>,
    pub cpu_total_percent: Option<f32>,
    #[serde(default)]
    pub cpu_per_core_percent: Vec<f32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub process_count: Option<usize>,
    pub disk: DiskSample,
    #[serde(default)]
    pub disk_devices: Vec<DiskDeviceSample>,
    #[serde(default)]
    pub network: NetworkSample,
    pub top_processes_cpu: Vec<ProcessSample>,
    pub top_processes_memory: Vec<ProcessSample>,
    pub marker_flags: MarkerFlags,
}

/// Per-device disk sample used for drill-down in the watch UI.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiskDeviceSample {
    pub key: String,
    pub label: String,
    pub read_bytes_per_sec: Option<f64>,
    pub write_bytes_per_sec: Option<f64>,
    pub busy_percent: Option<f32>,
    pub avg_latency_ms: Option<f32>,
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub file_system: Option<String>,
}

/// Process-level sample used for evidence and offender ranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSample {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub io_read_bytes_per_sec: Option<f64>,
    pub io_write_bytes_per_sec: Option<f64>,
    pub is_marker: bool,
    pub is_browser: bool,
    pub is_dev_tool: bool,
}

/// Disk-related sample values.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiskSample {
    pub read_bytes_per_sec: Option<f64>,
    pub write_bytes_per_sec: Option<f64>,
    pub busy_percent: Option<f32>,
    pub avg_latency_ms: Option<f32>,
}

/// Network-related sample values.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkSample {
    pub down_bytes_per_sec: Option<f64>,
    pub up_bytes_per_sec: Option<f64>,
    pub interface_count: Option<usize>,
    pub active_interface_count: Option<usize>,
}

/// Marker process flags per sample.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MarkerFlags {
    pub msmpeng_present: bool,
    pub search_indexer_present: bool,
    pub tiworker_present: bool,
    pub vmmem_present: bool,
    pub docker_present: bool,
    pub browser_process_count: usize,
    pub dev_tool_process_count: usize,
}

/// All supported diagnosis categories.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisKind {
    CpuSaturation,
    MemoryPressure,
    DiskContention,
    BackgroundScan,
    UpdateActivity,
    BrowserBloat,
    DevToolStorm,
}

impl DiagnosisKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CpuSaturation => "cpu_saturation",
            Self::MemoryPressure => "memory_pressure",
            Self::DiskContention => "disk_contention",
            Self::BackgroundScan => "background_scan",
            Self::UpdateActivity => "update_activity",
            Self::BrowserBloat => "browser_bloat",
            Self::DevToolStorm => "dev_tool_storm",
        }
    }
}

/// Evidence used for one diagnosis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub label: String,
    pub detail: String,
}

/// Actionable recommendation for users.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub action: String,
    pub rationale: String,
}

/// A ranked diagnosis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnosis {
    pub kind: DiagnosisKind,
    pub confidence: f32,
    #[serde(default)]
    pub duration_seconds: Option<f32>,
    pub explanation: String,
    pub evidence: Vec<Evidence>,
    pub suggestions: Vec<Suggestion>,
    pub partial_evidence: bool,
}

/// Aggregated process-family summary over the sample window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffenderSummary {
    /// Stable family key used for grouping related processes.
    pub family: String,
    /// Human-readable display name for this grouped offender.
    pub name: String,
    /// PID of the most impactful member process in this family, if known.
    pub representative_pid: Option<u32>,
    /// Count of unique processes observed in the family.
    pub process_count: usize,
    /// Up to a few member process names for visibility.
    pub member_names: Vec<String>,
    pub avg_cpu_percent: f32,
    pub peak_cpu_percent: f32,
    pub avg_memory_bytes: u64,
    pub peak_memory_bytes: u64,
    pub avg_io_read_bytes_per_sec: f64,
    pub avg_io_write_bytes_per_sec: f64,
    /// Number of samples where at least one family member was present.
    pub sample_hits: usize,
    pub marker_labels: Vec<String>,
}

/// High-level system summary for the report.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemSummary {
    pub avg_cpu_percent: Option<f32>,
    pub peak_cpu_percent: Option<f32>,
    pub avg_memory_used_percent: Option<f32>,
    pub peak_memory_used_percent: Option<f32>,
    pub avg_process_count: Option<f32>,
    pub avg_disk_read_bytes_per_sec: Option<f64>,
    pub avg_disk_write_bytes_per_sec: Option<f64>,
    pub avg_disk_busy_percent: Option<f32>,
    pub peak_disk_busy_percent: Option<f32>,
    pub avg_disk_latency_ms: Option<f32>,
    pub peak_disk_latency_ms: Option<f32>,
    pub avg_network_down_bytes_per_sec: Option<f64>,
    pub peak_network_down_bytes_per_sec: Option<f64>,
    pub avg_network_up_bytes_per_sec: Option<f64>,
    pub peak_network_up_bytes_per_sec: Option<f64>,
}

/// Internal analysis output before final report assembly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub summary: SystemSummary,
    pub diagnoses: Vec<Diagnosis>,
    pub top_offenders: Vec<OffenderSummary>,
}

/// Stable report schema for text and JSON output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: String,
    pub generated_at: DateTime<Utc>,
    pub config: RunConfig,
    pub host: HostInfo,
    pub sample_count: usize,
    pub sample_window_secs: u64,
    pub summary: SystemSummary,
    pub diagnoses: Vec<Diagnosis>,
    pub top_offenders: Vec<OffenderSummary>,
    pub unavailable_metrics: Vec<String>,
}

/// Clamp confidence scores to `[0.0, 1.0]`.
pub fn clamp_score(score: f32) -> f32 {
    score.clamp(0.0, 1.0)
}
