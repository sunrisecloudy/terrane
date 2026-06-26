# Review 087 - commits 6083b8cf, 5980cff2, d4b0f4ac

## Findings

1. [P3] Finish the sync fixture count cleanup in the delegation notes.

   Commit `6083b8cf` fixes the main Done table to say the sync convergence suite
   has 10 canonical scenarios, but the same file still says `T026 - 11 scenarios`
   in the Codex delegation pipeline (`prd-merged/FEATURE_COVERAGE.md:150`). That
   keeps the contradiction from review 085 alive for anyone using the backlog
   summary instead of the table. Please change that final T026 note to 10.

## Notes

- `5980cff2` only delegates T028. I handled that handoff in this run by adding
  `forge/spec/files.md` and the `forge/fixtures/files/` vector suite.
- `d4b0f4ac` addresses review 084 #1 by threading per-store `IndexManager`s
  through `sync_stores`, updating the `WorkspaceCore::sync_with` call site, and
  adding an asymmetric FTS regression. I did not find a new actionable issue in
  that diff.
