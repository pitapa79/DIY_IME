# Security reporting and verification

This repository is a prototype. In-process Rust Nodes are trusted code.
The external Node sandbox, protected Windows IME input, package signing, and a public support policy are not implemented by this document.

Do not publish passwords, API keys, real input history, private files, or working exploits in public issues.
Use GitHub private vulnerability reporting if the repository owner has enabled it.
If no private channel is available, ask the repository owner for a private reporting route without including sensitive details.
This document does not claim that private reporting is currently enabled or promise a response time.

A report should identify the tested commit, operating system, affected boundary, expected and observed behavior,
and a minimal reproduction using fake input/canary data. Keep evidence limited to what is necessary.

The maintainer should:

1. Acknowledge through the chosen private route and record a minimal, access-limited report.
2. Reproduce on an isolated test setup with canaries; classify the affected trust boundary and supported configurations.
3. Link the private fix task and add a regression reproducing the failure without using real secrets.
4. Review the fix independently, rerun the relevant gates, and confirm source/binary provenance.
5. Decide release and disclosure timing with the reporter; signing, supported releases, SLA, and publication policy require separate owner decisions.

The [security verification notes](docs/security/ci-audit.md) describe current CI and its limits.
Passing dependency scans or tests does not prove arbitrary third-party code is safe.
