use clap::{ArgAction, Parser};

use crate::model::RunConfig;

const AFTER_HELP: &str = "Examples:\n  whyslow\n  whyslow --json\n  whyslow --watch 5\n  whyslow --duration 40 --top 3\n  whyslow --json --export report.json\n";

/// Diagnose why this machine is slow right now.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "whyslow",
    version,
    about = "Diagnose why your Windows PC feels slow",
    long_about = "Sample system metrics over time, rank likely bottlenecks, show evidence, and suggest actions.",
    after_help = AFTER_HELP,
    arg_required_else_help = false,
    disable_help_subcommand = true,
    next_line_help = true
)]
pub struct Cli {
    /// Output report as stable JSON.
    #[arg(long)]
    pub json: bool,

    /// Re-run diagnostics every N seconds (interactive TUI unless --json).
    #[arg(long, value_name = "seconds", value_parser = parse_watch)]
    pub watch: Option<u64>,

    /// Sampling interval in milliseconds (100..10000).
    #[arg(long, value_name = "ms", default_value_t = 2000, value_parser = clap::value_parser!(u64).range(100..=10_000))]
    pub interval: u64,

    /// Number of diagnoses/offenders to display (1..20).
    #[arg(long, default_value_t = 5, value_parser = parse_top)]
    pub top: usize,

    /// Sampling window duration in seconds (5..300).
    #[arg(long, value_name = "seconds", default_value_t = 20, value_parser = clap::value_parser!(u64).range(5..=300))]
    pub duration: u64,

    /// Optional export output path (`.json` or `.md`).
    #[arg(long, value_name = "path")]
    pub export: Option<String>,

    /// Increase logs (`-v`, `-vv`).
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    pub verbose: u8,
}

impl Cli {
    pub fn to_config(&self) -> RunConfig {
        RunConfig {
            sample_window_secs: self.duration,
            interval_ms: self.interval,
            top_n: self.top,
            json_output: self.json,
            watch_seconds: self.watch,
            export_path: self.export.clone(),
            verbose: self.verbose,
        }
    }
}

fn parse_watch(value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| "watch must be an integer between 1 and 3600".to_string())?;

    if (1..=3600).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("watch must be between 1 and 3600 seconds".to_string())
    }
}

fn parse_top(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| "top must be an integer between 1 and 20".to_string())?;

    if (1..=20).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("top must be between 1 and 20".to_string())
    }
}
