# Linux Durable Forge Open - Independent Review

## Slice goal

Fix the Linux native host persistence blocker by opening Forge through
file-backed `forge_core_open(path, workspace_id)` instead of
`forge_core_open_in_memory`.

## Files changed

- `native/linux/src/forge_core_bridge.c`

## Resolution

- The Linux host now resolves `forge_core_open`.
- The bridge creates the user data directory `Terrane` with `0700`
  permissions and opens `forge-workspace.sqlite` inside it.
- The old `forge_core_open_in_memory` use was removed from the Linux bridge.

## Verification

- `rg "open_in_memory|forge_core_open_in_memory|ForgeCoreOpenInMemory" native/linux/src/forge_core_bridge.c`: no matches.
- `git diff --check -- native/linux/src/forge_core_bridge.c`: passed.
- `meson setup builddir` in `native/linux`: not run; `meson` is not installed
  on this machine.
- `pkg-config --cflags glib-2.0 json-glib-1.0 gtk4 webkitgtk-6.0 sqlite3`: not
  available for the Linux native stack here; `json-glib-1.0`, `gtk4`, and
  `webkitgtk-6.0` were missing.

## Follow-up

- Run the native Linux Meson build in a Linux environment with the GTK/WebKit
  development packages installed.
