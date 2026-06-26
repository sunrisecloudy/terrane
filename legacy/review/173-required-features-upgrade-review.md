# Review: 2e7c2bad required-features upgrade gate

## Findings
- No actionable findings. The commit runs MP-8 `required_features` negotiation
  during `applet.upgrade` staging before signature verification, so the review-170
  signed `compatibility` bind is now fail-closed on both signed install entry
  points. The new tests cover unsupported signed upgrade refusal, active-version
  rollback/audit, and a supported signed upgrade commit.

## Checks
- `cargo test -p forge-core --test spine signed_required_feature --offline`
- `cargo test -p forge-core --test required_features_vectors --offline`
