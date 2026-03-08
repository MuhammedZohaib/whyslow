pub mod cli;
pub mod collect;
pub mod diagnose;
pub mod model;
pub mod report;

use anyhow::Result;

use crate::model::{Report, RunConfig};

/// Runs a single sample/diagnosis/report cycle.
pub fn execute_once(config: &RunConfig) -> Result<Report> {
    let window = collect::collect_window(config)?;
    let analysis = diagnose::analyze(config, &window);
    Ok(report::build_report(config.clone(), window, analysis))
}
