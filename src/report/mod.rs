pub mod json;
pub mod markdown;
pub mod text;
pub mod tui;

use chrono::Utc;

use crate::model::{AnalysisResult, CollectionWindow, Report, RunConfig};

/// Build the final report object from raw collection and analysis output.
pub fn build_report(config: RunConfig, window: CollectionWindow, analysis: AnalysisResult) -> Report {
    let sample_window_secs = (window.ended_at - window.started_at)
        .to_std()
        .map(|d| d.as_secs())
        .unwrap_or(config.sample_window_secs);

    Report {
        schema_version: "1.0.0".to_string(),
        generated_at: Utc::now(),
        config,
        host: window.host,
        sample_count: window.samples.len(),
        sample_window_secs,
        summary: analysis.summary,
        diagnoses: analysis.diagnoses,
        top_offenders: analysis.top_offenders,
        unavailable_metrics: window.unavailable_metrics,
    }
}
