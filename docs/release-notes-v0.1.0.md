# v0.1.0 Release Notes

## What Works

- Windows-first diagnostics for CPU, memory, disk, and process-family offenders
- Ranked bottleneck output with confidence and evidence
- Suggested actions for each diagnosis
- JSON output for automation (`--json`)
- Watch mode for repeated checks (`--watch`)
- Export to JSON/Markdown (`--export`)

## Known Limitations

- Windows-first support only
- Some metrics may be unavailable depending on permissions/counters
- Confidence scores are heuristic and not a guarantee

## Next Steps

- Add network bottleneck diagnosis
- Expand per-process disk I/O reporting
- Improve confidence scoring calibration
- Add WSL-specific diagnostics
