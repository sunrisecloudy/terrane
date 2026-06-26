# Commit Review 107

Reviewed commit: `44626c57` (`collab(codex): T036 applet lifecycle spec + 13 vectors (CR-7)`)

## Findings

No actionable findings.

The commit adds the requested T036 lifecycle contract in `forge/spec/applet-lifecycle.md` and 13 semantic vectors under `forge/fixtures/lifecycle/`. The pack covers install -> enabled, enable/run/dispatch, suspended dispatch rejection, re-enable, atomic upgrade success and rollback, replay pinned to the recorded `code_hash`, uninstall keep-data vs purge-data, illegal run after uninstall, same-payload reinstall no-op, idempotent suspend, and fresh generation after reinstall.

## Notes For Wiring

- `applet.enable` is intentionally specified as a needed lifecycle command even though it is not yet in `forge/spec/commands.md`; add it when wiring the state machine.
- Current `cmd_applet_install` bumps version on any reinstall; the T036 vector `reinstall_same_code_hash_noop.json` intentionally pins the future behavior where same manifest/source/code hash is idempotent and different active payloads go through `applet.upgrade`.

## Verification

- `jq empty forge/fixtures/lifecycle/*.json`
