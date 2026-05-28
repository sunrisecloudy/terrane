# `examples/` — deprecated duplicate

> **Canonical path:** `webapps/examples/`
>
> This directory is a synchronized copy of `webapps/examples/` retained for backward compatibility with earlier tooling. `docs/02_PROJECT_STRUCTURE.md` and `docs/05_NATIVE_PLATFORM_REQUIREMENTS.md §9` declare `webapps/examples/` as the canonical location.

## Plan

Implementation will pull example packages from `webapps/examples/` only. This `examples/` tree will be removed when CI lands a manifest-sync check confirming all consumers have switched. Track removal in `IMPLEMENTATION_STATUS.md`.

## Don't edit one tree without the other

Until removal, every edit to a file under `webapps/examples/` must be mirrored here, and vice versa. `SPEC_VALIDATION_REPORT.md` verifies that the two trees are byte-identical.

Tools should prefer `webapps/examples/` as input. CI should fail if a file in this tree is newer than its `webapps/examples/` counterpart.
