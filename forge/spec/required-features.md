# Required features / capability negotiation (MP-8)

Source of record: `prd-merged/08-marketplace-prd.md` **MP-8** (a package declares
`required_features`; the client uses capability negotiation to refuse or run
limited-mode gracefully) and **MP-4** (the package
`compatibility{min_app_version, required_features}` field), composed with the
signed-package unknown-field fail-closed gate (`forge/crates/core/src/signing.rs`,
reviews 086/089). This note is the semantic contract for the vectors in
`forge/fixtures/required-features/`; it is not a wire format.

> **MP-8.** Compatibility: packages declare `required_features`; clients use
> capability negotiation to refuse or limited-mode gracefully.

## The manifest field

A package's compatibility floor rides on the manifest as
`compatibility{ min_app_version?, required_features[] }`
(`forge_domain::Compatibility`):

```json
"compatibility": {
  "min_app_version": "1.2.0",
  "required_features": [
    { "feature_id": "ctx.db.query", "min_version": "1.0.0" },
    { "feature_id": "ui.tabs", "min_version": "0" }
  ]
}
```

- **`required_features[]`** — a list of `{ feature_id, min_version }`. `feature_id`
  is a stable capability/runtime feature identifier (e.g. `ctx.db.query`,
  `ctx.net.fetch`, `ui.tabs`); `min_version` is a dotted-numeric version the
  package needs (`"1"`, `"1.2"`, `"1.2.0"` — trailing zeros do not matter). An
  omitted `min_version` defaults to `"0"` ("any supported version"). Empty (the
  default) → no required features → the package installs on any client.
- **`min_app_version`** — the minimum host app version, negotiated as the synthetic
  feature id `app` (so a too-old host is enumerated alongside any feature gap).
- Both fields default empty, so every existing manifest (the spine demo, the
  lifecycle fixtures) declares no floor and installs unchanged. The whole
  `compatibility` object is preserved verbatim across re-encodings (DL-9 habit), so
  a future client that adds a feature to its registry can install a package an
  older client refused without the package changing.

A malformed requirement — a blank `feature_id`, or a non-dotted-numeric
`min_version` / `min_app_version` — is a structural `ValidationError` at
`Manifest::validate`, before negotiation runs.

## The client feature registry (TRUSTED state)

The installing client holds a `ClientFeatureRegistry`: the set of
`feature_id -> supported_version` it supports. This is **trusted workspace state**,
NOT request payload — exactly like the SC-10 run policy / the `db.read` grant table
(review 048/050). A package (or a shell) cannot widen what the client claims to
support by editing the command body.

- The deterministic built-in baseline (`ClientFeatureRegistry::current`) advertises
  the features THIS build actually implements/enforces: the synthetic `app` id at
  the running app version; the live capability/runtime surfaces a package may
  require today (`ctx.db.query`, `ctx.db.watch`, `ctx.net.fetch`, `ctx.files`,
  `ctx.secrets`, `ctx.ui`, `signing.ed25519`); and the signed-policy fields the
  unknown-field gate can enforce, under the `signed.policy.*` namespace.
- A host MAY extend/replace the registry through the trusted
  `WorkspaceCore::set_client_feature_registry` seam; it is persisted to the
  workspace file so a host-extended registry survives reopen. An un-provisioned
  workspace uses the baseline, so the gate is **live by default**.
- Feature ids are **normalized** before comparison: trim surrounding whitespace +
  ASCII-lowercase, applied symmetrically to BOTH the package's required ids and the
  registry keys. So a package's `CTX.DB.Query` and a client's `ctx.db.query` name
  the same feature. The normalization is deterministic and locale-independent
  (feature ids are ASCII identifiers).

## The install rule

On `applet.install`, **before** accepting the install (before the signature check,
before any state is touched), the client negotiates the manifest's `compatibility`
against the trusted registry:

> Install **only if** the client supports **every** required feature at a version
> `>=` its `min_version` (and meets `min_app_version`). Otherwise **refuse**.

- A required feature the client does **not know** (missing), or knows only at a
  **lower** version than required, is **unsupported**.
- Version comparison is dotted-numeric, component-wise, missing component = `0`
  (so `1.3` ≥ `1.2.9` and `1.2` ≥ `1.2.0`). A version that fails to parse is
  fail-closed — never "at least" the requirement.
- A refusal is a typed `ValidationError` whose message **enumerates EVERY**
  unsupported feature — not just the first — each with its required min and what the
  client has (`"<id> (required >= <min>, client has <have>|none)"`). Nothing is
  stored.
- An empty `required_features` (and no `min_app_version`) installs. A client that
  supports a **superset** at higher versions installs (forward-compat).

The decision is read ONLY from the trusted registry and is **deterministic**, so
the demo replays identically.

## Composition with the signed-package fail-closed gate (reviews 086/089)

The signed-install path (`forge/crates/core/src/signing.rs`,
`reject_unknown_signed_policy_fields`) fails **closed** on any UNKNOWN signed policy
field this core cannot enforce. MP-8 is the matching **accept** side, and the two
gates AGREE on the same fact — *does this client support the feature?*

- A signed **future** policy field is admissible only if the package **declares** it
  in `required_features` **and** the client supports it. The package advertises the
  field under the `signed.policy.*` feature namespace; the client's registry lists
  the `signed.policy.*` ids it can enforce.
- A signed future field that is **NOT declared** in `required_features` is refused:
  the signature gate rejects the unknown field (the negotiation gate never had a
  chance to admit it).
- A signed future field that **IS declared** but the client does **not support** is
  refused by the negotiation gate, before the signature gate runs.

So a package can only carry a future signed constraint that this client actually
enforces — never a silent, unenforced one. The negotiation runs first (it does not
depend on a signature), then the signature gate; both must pass.

## Vectors

`forge/fixtures/required-features/manifest.json` declares the suite; each
`<case>.json` carries `required_features` + the client's supported set + the
expected `install`/`refuse` decision and (on refuse) the enumerated unsupported
feature ids. The data-driven harness
(`forge/crates/core/tests/required_features_vectors.rs`) drives each case through a
real `WorkspaceCore::handle("applet.install", …)` with the case's client registry
installed, asserting the install succeeds or is refused naming every unsupported
feature, and `ran == count` keeps the corpus honest.

| Case | required_features (client) | Expected |
|---|---|---|
| `all_supported_installs` | every required feature in the client set | install |
| `one_unsupported_refuses` | one required feature missing from the client | refuse, naming it |
| `min_version_higher_than_client_refuses` | required min > the client's version | refuse (client-has stated) |
| `empty_required_features_installs` | none required | install |
| `signed_policy_field_declared_supported_installs` | a `signed.policy.*` field declared + client supports it | install |
| `signed_policy_field_undeclared_refuses` | the signed future field NOT declared in required_features | refuse |
| `multiple_unsupported_lists_all` | several required features unsupported | refuse, listing ALL |
| `feature_id_case_normalization_installs` | required/client ids differ only by case | install |
| `forward_compat_superset_installs` | client supports a superset at higher versions | install |
| `min_app_version_too_low_refuses` | `min_app_version` above the client app version | refuse, naming `app` |
