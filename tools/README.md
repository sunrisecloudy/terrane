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

`package-release` is implemented as `tools/package-release.mjs`. It writes the `docs/12` release artifact tree: deterministic ZIP archives for `runtime-web/` and `webapps/examples/`, optional `--build-zig-core` target libraries, optional `--build-server` host-native server executable output, target-output directories for native jobs, and `release-manifest.json` with sizes and hashes.
