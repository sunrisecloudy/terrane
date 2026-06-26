# Native Persistence Static Check - Independent Review

## Slice goal

Update the repository static checker after the native host persistence fixes so
it requires durable `forge_core_open` usage and rejects reintroduced
`forge_core_open_in_memory` usage in native host bridges.

## Files changed

- `tools/check-repo.mjs`

## Resolution

- macOS, iOS, Linux, Android, and native Windows static checks now require
  `forge_core_open`.
- Each native host check now fails if the corresponding bridge source contains
  `forge_core_open_in_memory`.

## Verification

- `node --no-warnings tools/check-repo.mjs`: passed.
