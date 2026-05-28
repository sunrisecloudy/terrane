# Implementation Guardrails

## Do not change these without updating docs, schemas, examples, and tests

- Bridge method list.
- Bridge error shape.
- Manifest fields.
- Zig core FFI function names.
- Generated package file names.

## Generated webapp constraints

- No build step.
- No external dependencies.
- No direct fetch.
- No localStorage/IndexedDB/cookies.
- No direct native bridge access.
- Use only `AppRuntime.call`.

## Native shell constraints

- Native shells are adapters, not product logic containers.
- Native shells must do permission checks even if web runtime already did.
- Native shells must return structured errors.
- Native shells must not expose raw file paths to generated apps in v0.1.

## Zig constraints

- Keep core deterministic.
- Keep v0.1 JSON-based.
- Return logical errors as JSON.
- Do not crash on bad input.
- Use explicit allocators internally.
