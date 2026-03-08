use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use tracing::{error, info};

use whyslow::cli::Cli;
use whyslow::model::RunConfig;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = cli.to_config();
    init_tracing(config.verbose);

    if let Some(watch_seconds) = config.watch_seconds {
        if config.json_output {
            run_watch_loop_json(config, watch_seconds)
        } else {
            whyslow::report::tui::run_watch(config, watch_seconds)
        }
    } else {
        run_and_print(&config)
    }
}

fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn run_watch_loop_json(config: RunConfig, watch_seconds: u64) -> Result<()> {
    let cadence = Duration::from_secs(watch_seconds.max(1));
    let mut next_run = Instant::now();

    loop {
        if let Err(err) = run_and_print(&config) {
            error!(error = %err, "run failed");
            eprintln!("whyslow run failed: {err:#}");
        }

        // Respect `--watch` as run-to-run cadence, not `duration + watch`.
        next_run += cadence;
        let now = Instant::now();
        if next_run > now {
            let sleep_for = next_run - now;
            info!(
                watch_seconds,
                sleep_ms = sleep_for.as_millis() as u64,
                "sleeping before next run"
            );
            thread::sleep(sleep_for);
        } else {
            // If a run took longer than cadence, trigger next run immediately and rebase.
            next_run = now;
        }
    }
}

fn run_and_print(config: &RunConfig) -> Result<()> {
    let report = whyslow::execute_once(config)?;

    if config.json_output {
        println!("{}", whyslow::report::json::render(&report)?);
    } else {
        println!("{}", whyslow::report::text::render(&report));
    }

    if let Some(path) = config.export_path.as_deref() {
        export_report(path, &report, config)?;
    }

    Ok(())
}

fn export_report(path: &str, report: &whyslow::model::Report, config: &RunConfig) -> Result<()> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "json".to_string());

    let content = if ext == "md" || ext == "markdown" {
        whyslow::report::markdown::render(report)
    } else if ext == "json" || config.json_output {
        whyslow::report::json::render(report)?
    } else {
        whyslow::report::markdown::render(report)
    };

    fs::write(path, content)?;
    info!(path, "exported report");
    Ok(())
}
