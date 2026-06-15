# Closure Blockers - Independent Audit

## Slice goal

Record the remaining closure blockers after the initial Forge cutover commit
chain, before continuing with host persistence fixes.

User direction update: use subagents heavily, commit very often, and keep commit
boundaries small enough for Claude to review independently.

## Findings

- Native host persistence is still a blocker. The surviving native hosts load
  Forge through `forge_core_open_in_memory`, which loses Forge workspace state
  across restart. This matches the existing untracked blocker note in
  `review-from-claude/027-host-persistence-regression-CONFIRMED.md`.
- A literal final review artifact is still missing. The earlier independent run
  recorded slice evidence, but did not commit
  `review-from-claude/FINAL-legacy-removal-forge-cutover.md` or an equivalent
  committed final independent closure note.
- CI/release closure is local-evidence only. Local Forge, reference-host,
  package, Android, and contract checks passed, but there is no committed record
  of remote green `forge-ci.yml`, `ci.yml`, or `release.yml` evidence.
- Forge server `/bridge` still accepts a caller-supplied `CoreCommand` directly.
  That remains a security/parity follow-up before treating the Forge server as a
  network-exposed replacement.
- Broad legacy-reference audit found no actionable committed live Zig build or
  runtime references. Remaining hits were ignored `.forge-wf` prompt strings and
  low-risk status documentation.

## Resolution plan

- Fix native persistence in small host-scoped commits, starting with the common
  macOS/iOS C wrapper pattern.
- Add focused persistence smoke coverage where practical.
- Keep each follow-up commit narrowly staged and reviewable.
- Leave remote CI/release evidence as a recorded external verification gap until
  a real remote run is available.
