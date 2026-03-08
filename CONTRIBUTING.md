# Contributing to whyslow

Thanks for contributing to `whyslow`. This project aims to provide clear, local, Windows-first performance diagnosis in a terminal workflow.

## Development Setup

1. Install Rust stable (`rustup default stable`).
2. Clone the repository.
3. Build and run:

```powershell
cargo run -- --duration 20
```

## Required Checks

Before opening a PR, run all checks locally:

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

PRs that skip these checks are usually blocked in CI.

## Module Structure

- `src/main.rs`: CLI entrypoint, watch loop, export handling
- `src/cli.rs`: clap arguments, help text, and run config mapping
- `src/collect/`: sampling and platform collectors (Windows PDH/sysinfo)
- `src/diagnose/`: deterministic diagnosis engine and rule scoring
- `src/report/`: text, markdown, JSON, and watch/TUI rendering
- `src/model.rs`: shared report/schema/domain structs
- `tests/`: rule behavior and output snapshots

## Coding Style

- Keep functions focused and side-effect boundaries clear.
- Prefer explicit names over abbreviations in diagnosis/report paths.
- Keep user-facing strings concise and actionable.
- Add tests for behavior changes, not just happy-path examples.

## Diagnosis Engine Overview

`whyslow` uses deterministic, local heuristics.

1. Collect a sampling window (`CollectionWindow`).
2. Compute a `SystemSummary` from sample series.
3. Group process samples into offender families.
4. Score diagnosis rules independently (confidence in `[0.0, 1.0]`).
5. Sort diagnoses by confidence, truncate to `top_n`.
6. Render text/JSON/markdown outputs.

Confidence is heuristic and built from ramps and weighted factors. There is no cloud model, no remote inference, and no auto-upload.

## Adding a New Diagnosis Rule

1. Add a function in `src/diagnose/rules.rs` with signature:
   - `fn rule_name(window, summary, offenders) -> Option<Diagnosis>`
2. Build confidence from measurable evidence and `ramp(...)`/`clamp_score(...)`.
3. Add clear evidence lines and at least one actionable suggestion.
4. Set `partial_evidence = true` when key counters are missing.
5. Register the rule in `src/diagnose/mod.rs` so it runs and is ranked.
6. Add tests under `tests/`:
   - positive detection
   - non-trigger case
   - edge case with partial metrics

## Testing Rule Changes

- Add or update fixtures for report output when user-facing text changes.
- Keep snapshot changes intentional and easy to review.
- For false positives/negatives, include reproducer data when possible.

## Screenshots and Repro Traces

For visual/reporting changes, include:

- one terminal screenshot (tight crop, readable text)
- command used (for example `whyslow --duration 30 --json`)
- optional exported JSON sample (sanitized)

Store non-sensitive examples in `docs/examples/`.

## Pull Request Process

- Fill out `.github/PULL_REQUEST_TEMPLATE.md`.
- Explain what changed and why.
- Note how you tested it.
- Mention any behavior changes in `CHANGELOG.md` under `Unreleased`.
