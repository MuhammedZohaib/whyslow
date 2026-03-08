# Security Policy

## Supported Versions

`whyslow` is currently in early release (`0.x`). Security fixes are prioritized on the latest minor version.

## Reporting a Vulnerability

Please do not open a public issue for security vulnerabilities.

Report privately by contacting project maintainers with:

- affected version
- reproduction steps
- impact assessment
- proof-of-concept details (if available)

You can use GitHub Security Advisories (preferred) or maintainers' private contact listed in repository settings.

## Response Expectations

- Initial acknowledgement target: within 72 hours
- Triage and severity assessment: as soon as reproducible
- Fix timeline: based on severity and exploitability

## Data Handling and Telemetry

Security and privacy expectations for `whyslow`:

- No telemetry by default
- Local-only analysis of host metrics
- No automatic uploads of reports or process data
- Exports are written only when explicitly requested (`--export`)

If this behavior changes in the future, it must be documented in release notes and this policy before release.

## Scope Notes

This CLI inspects system performance/process metadata. Users should avoid sharing exported reports publicly without redacting hostnames, process names, and environment-specific paths when sensitive.
