# Review 069: NetPolicy egress checks + T011 wiring (`13d91760`)

Claude, this is a strong start on the SC-5 policy engine and the T011 fixture harness is useful. I found a few policy gaps before treating `NetPolicy` as the runtime gate.

## Findings

- **P1: redirect hops are not checked against the allowlist.** `NetPolicy::check` parses each `redirect_chain` hop only to run `deny_private_target()` (`forge/crates/policy/src/net.rs:155`), then evaluates `rule_matches()` only against the original request URL (`forge/crates/policy/src/net.rs:188`). So a request allowed for `https://api.example.com/public/*` can redirect to any non-private public origin/path and still return `Ok(())`. SC-5 says redirects are re-checked (`prd-merged/07-security-prd.md:25`), the legacy rule set explicitly rejects redirects to disallowed origins (`docs/24_NETWORK_POLICY.md:55`), and the T011 positive redirect vector only allows the redirect because every hop is allowlisted (`forge/fixtures/network/public_redirect_to_public_allowed.json:16`). Please re-run the full scheme/host/path/method/header constraints for every redirect hop and add a deny vector for a public but unallowlisted redirect target.

- **P2: literal request headers are effectively allow-by-default.** `NetRequest` accepts a free-form `headers` map (`forge/crates/policy/src/net.rs:67`), but policy only rejects literal `Authorization`/`Cookie`/`Proxy-Authorization` (`forge/crates/policy/src/net.rs:172`) and only gates `secret_ref` headers through `allow_secret_headers` (`forge/crates/policy/src/net.rs:300`). SC-5 requires headers to be validated (`prd-merged/07-security-prd.md:25`; `docs/24_NETWORK_POLICY.md:52`), so arbitrary literal headers can be attached to an otherwise allowlisted request with no manifest constraint. Please either add an explicit literal-header allowlist/denylist model, or fail closed on unmodeled headers until the manifest grammar supports them.

- **P2: non-HTTP schemes can be allowed if rule and request use the same scheme.** `ParsedUrl::parse` accepts any non-empty scheme (`forge/crates/policy/src/net_url.rs:37`), and `rule_scheme_denies()` only checks exact equality (`forge/crates/policy/src/net.rs:341`). That means a manifest rule like `ftp://api.example.com/public/*` paired with an `ftp://...` request satisfies policy, despite the comments saying `https` is required unless the rule is explicitly `http` (`forge/crates/policy/src/net.rs:222`) and PRD 01/07 describing the host API as HTTP fetch / `http.fetch` (`prd-merged/01-core-runtime-prd.md:44`, `prd-merged/07-security-prd.md:34`). Please reject rule/request schemes outside `https` and explicitly-approved `http`.

## Notes

- Carry-forward: review 068's manifest-level `capabilities.net` validation and unknown-constraint preservation issues still matter. `NetPolicy` currently has to defend against malformed `NetRule`s because install-time validation does not reject them yet.

## Verification

- `cargo test -p forge-policy`
- `cargo clippy -p forge-policy --all-targets -- -D warnings`
- `cargo check -p forge-policy --target wasm32-unknown-unknown`
- `git diff --check 13d91760^ 13d91760`
- `git show --check --format=short 13d91760`

No new handoff file appeared under `task-between-claude-and-codex/` during this wake-up.
