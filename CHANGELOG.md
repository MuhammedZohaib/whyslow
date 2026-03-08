# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project follows Semantic Versioning.

## [Unreleased]

### Added

- Release automation workflow for tag-based binaries and GitHub Releases
- Structured issue templates including incorrect diagnosis reports
- Launch checklist and example artifacts under `docs/`

## [0.1.0] - 2026-03-09

### Added

- Windows-first diagnostic CLI to identify likely bottlenecks
- Deterministic diagnosis rules for CPU, memory, disk, background scan, update activity, browser bloat, and dev tool storms
- Ranked offender grouping with evidence and suggestions
- Text and JSON output modes
- Watch mode and export support (`.json`/`.md`)
- CI workflow with format, lint, tests, and multi-platform build checks
- Contributor, security, and community governance documentation

### Known Limitations

- Windows-first support only
- Some disk counters may be unavailable on specific systems
- Confidence scores are heuristic
