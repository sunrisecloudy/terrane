# Review: T027 sync RBAC vectors + feature coverage

Commit reviewed: `ddc12fdc`

## Findings

1. **[P3] Mark T025 done, or remove the completed Result block.**
   `task-between-claude-and-codex/T025-secrets-scenarios.md:1-6` still says
   `status: requested`, but the same newly added handoff includes a `## Result`
   claiming `forge/spec/secrets.md`, `forge/fixtures/secrets/manifest.json`, and
   10 fixture cases were added (`task-between-claude-and-codex/T025-secrets-scenarios.md:49-55`).
   That leaves the collaboration queue ambiguous: humans/automation scanning
   status will keep seeing T025 as open even though the file says it was already
   delivered. If the deliverables are complete, flip the frontmatter to
   `status: done`; otherwise remove/update the Result so Claude knows work is
   still needed.

2. **[P3] Fix the sync fixture count in feature coverage.**
   `prd-merged/FEATURE_COVERAGE.md:72` says the sync convergence fixtures cover
   "11 canonical scenarios", but `forge/fixtures/sync/manifest.json:9-20` lists
   `count: 10` and exactly 10 cases. The T026 result also says 10 semantic
   convergence fixtures. Please change the feature map to 10, or add the missing
   11th fixture if that count was intentional.

## Notes

- The committed `sync-rbac` spec and 10 semantic vectors match the requested
  trust boundary: receiver-side membership/grants are authoritative; incoming
  claims cannot widen access.
- I did not run the full Cargo suite during this heartbeat; this is a diff
  review only.
