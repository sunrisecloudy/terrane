# Tools Target

Codex should implement tools here.

Suggested tools:

```text
validate-webapp-package
package-examples
package-release
run-linux-native-docker
verify-public-contract
generate-fixtures
replay-core-events
```

The first required tool is `validate-webapp-package`, which validates manifest shape, package file list, permissions, storage prefix, and banned HTML/JS/CSS patterns.

`package-release` is implemented as `tools/package-release.mjs`. It writes the `docs/12` release artifact tree: deterministic ZIP archives for `runtime-web/` and `webapps/examples/`, optional `--build-forge-ffi` host Forge FFI library output, optional `--build-server` host-native Forge server executable output, optional `--build-native-macos` `.app` plus `.dmg` output with `libforge_ffi.dylib`, optional Linux-only `--build-native-linux` host output with `terrane-host`, `libforge_ffi.so`, runtime/example resources, and SQLite migrations, optional Windows-only `--build-native-windows` host output with `forge_ffi.dll`, target-output directories for remaining native jobs, and `release-manifest.json` with sizes and hashes.

`run-linux-native-docker` is implemented as `tools/run-linux-native-docker.mjs`. It builds `native/linux/Dockerfile`, mounts the repository read-only, and runs the Linux WebKitGTK native smoke test inside the container. On non-x64 hosts it defaults Docker to `linux/amd64` so the smoke matches the supported `linux-x86_64` release artifact target; pass `--platform` to override.

`verify-public-contract` is implemented as `tools/verify-public-contract.mjs`. It validates `public-contract.json` against `forge/contracts/public-contract.schema.json`, checks source commit provenance, and verifies every recorded public doc/contract/fixture/tool hash against the matching Terrane checkout.
