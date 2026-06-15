# Forge Example Applets

The Forge v1 example applets live under `forge/examples/`. They are TypeScript
applets that install through `applet.install`, run through `runtime.run`, render
Forge UI trees, and replay through `runtime.replay`.

These examples are the replacement set for the legacy build-free
`webapps/examples/` packages. Packaging and runtime consumers may still point at
the legacy tree during the cutover, but the Forge examples are now executable
through the core command facade.

| Example | Coverage |
|---|---|
| `notes-lite` | DB insert/list plus UI list rendering. |
| `task-workbench` | DB query, app-scoped storage, workflow-style task state. |
| `file-transformer` | Sandboxed `ctx.files.write`, DB transform summary, UI status. |
| `api-dashboard` | Manifest-gated `ctx.net.fetch`, DB request history, UI list. |
| `core-replay-lab` | Deterministic time/random seams, storage, DB, replay-friendly UI. |

The executable gate is:

```text
cargo test -p forge-cli --test forge_examples --locked
```

The test installs every example from disk, injects deterministic mock network and
filesystem seams where needed, runs it through `WorkspaceCore::handle`, checks
that it writes expected records and renders UI, then confirms
`runtime.replay` reports byte-identical replay.
