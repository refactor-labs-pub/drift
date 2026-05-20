# Security Policy

## Reporting a vulnerability

If you discover a security vulnerability in Drift, please report it
privately so we can fix it before public disclosure.

**Preferred channels** (any of the following):

- Email: **security@refactor-labs.io**
- GitHub Security Advisory: open a private advisory at
  <https://github.com/refactorlab/drift/security/advisories/new>

Please include:

- A description of the issue and the potential impact
- Steps to reproduce, or a minimal proof of concept
- The affected component (`drift-static-profiler`, `drift-lab`,
  `drift-observability/drift-profiler-python`, `web-app`, `action`, etc.)
- The version or commit SHA where you observed the issue
- Whether you intend to publish your own write-up, and if so on what
  timeline

## What to expect

| Stage                                 | Target time |
|---------------------------------------|-------------|
| Acknowledgement of your report        | 3 business days |
| Initial assessment + severity grading | 7 business days |
| Fix released for confirmed High/Critical issues | 30 days |
| Public disclosure / CVE coordination  | After a fix is available, in coordination with reporter |

We follow responsible disclosure. If a vulnerability is being actively
exploited or has been disclosed publicly elsewhere, we may compress the
above timelines.

## Scope

In scope:

- All first-party code in this monorepo
- The published `drift-lab` desktop app, `drift-static-profiler` CLI,
  `drift-docker-profiler` Python package, the Drift web-app, and the
  Drift GitHub Action

Out of scope:

- Third-party dependencies (please report those to the upstream project,
  but feel free to copy us)
- Social engineering of contributors or maintainers
- Denial-of-service against shared infrastructure not operated by
  Refactorlabs
- Issues that require physical access or already-compromised user accounts

## Safe harbor

We will not pursue legal action against researchers who:

- Make a good-faith effort to comply with this policy
- Do not exfiltrate or destroy user data
- Do not disrupt services beyond what is needed to demonstrate the issue
- Give us a reasonable window to remediate before public disclosure

Thank you for helping keep Drift and its users safe.
