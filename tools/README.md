# Tools Target

Codex should implement tools here.

Suggested tools:

```text
validate-webapp-package
package-examples
package-release
generate-fixtures
replay-core-events
```

The first required tool is `validate-webapp-package`, which validates manifest shape, package file list, permissions, storage prefix, and banned HTML/JS/CSS patterns.

`package-release` is implemented as `tools/package-release.mjs`. It writes the `docs/12` static release artifact tree: deterministic ZIP archives for `runtime-web/` and `webapps/examples/`, target-output directories for Zig/server/native jobs, and `release-manifest.json` with sizes and hashes.
