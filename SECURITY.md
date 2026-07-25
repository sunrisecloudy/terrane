# Security policy

## Supported releases

Only the latest production GitHub release receives security fixes. Preview,
unsigned validation builds, unverified local artifacts, and older releases are
unsupported.

## Reporting a vulnerability

Report vulnerabilities privately through
[GitHub Security Advisories](https://github.com/sunrisecloudy/terrane/security/advisories/new).
Do not include secrets, personal data, or an active exploit in a public issue.

Include the affected release, macOS version, architecture, reproduction steps,
impact, and any suggested mitigation. Maintainers should acknowledge a complete
report within seven days and coordinate remediation and disclosure based on
severity.

## Release trust

Official macOS releases are:

- attached to a versioned GitHub release in `sunrisecloudy/terrane`;
- Apple-silicon-only;
- signed with Apple Developer ID and hardened runtime;
- notarized by Apple;
- accompanied by SHA-256 checksums and a release manifest identifying the
  exact source commit.

If Gatekeeper rejects an official release or its checksum differs, do not run
it and report the discrepancy privately.
