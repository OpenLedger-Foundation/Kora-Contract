# Incident Response Templates

This directory contains pre-written communication templates for various incident scenarios. Use these to ensure consistent, professional, and timely communication during high-pressure events.

## Available Templates

- `sev1-pause.md`: For emergency contract pauses.
- `key-compromise.md`: For security-critical key rotation events.
- `indexer-lag.md`: For data availability or synchronization issues.
- `bounty-disclosure.md`: For coordinated vulnerability disclosure.

## Usage

Replace all `{{TOKEN}}` placeholders before sending. Ensure internal/external variants are selected based on the severity table in `INCIDENT_RESPONSE.md`.

## Variable Glossary

- `{{TIMESTAMP}}`: Current UTC time.
- `{{IMPACT_SUMMARY}}`: Brief description of affected functionality.
- `{{ETA}}`: Estimated time for next update or resolution.
- `{{INCIDENT_ID}}`: Unique tracking ID from the operations channel.