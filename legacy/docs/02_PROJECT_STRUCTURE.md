# Project Structure Pointer

> **Superseded:** The former v0.4 repository map was removed after the Forge
> cutover.

Current source layout:

- [`forge/`](../forge/) - Rust workspace for the v1 core, runtime, storage,
  policy, sync, server, FFI, CLI, and conformance surfaces.
- [`prd-merged/`](../prd-merged/) - normative v1 product requirements and
  decisions.
- [`native/`](../native/) - platform shells wired to Forge surfaces where
  supported.
- [`runtime-web/`](../runtime-web/) and [`webapps/`](../webapps/) - retained
  compatibility/runtime packages used by current host tests and release assets.
- [`tools/`](../tools/) - release, contract, and reference-host tooling.

For a status map of active and retained paths, see
[`IMPLEMENTATION_STATUS.md`](../IMPLEMENTATION_STATUS.md).
