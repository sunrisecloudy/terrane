# Review: 99b925ee signed compatibility binding

## Findings
- No actionable findings. The commit binds the signed package `compatibility`
  floor to the top-level manifest before accepting a signed install, closing the
  stripped-`required_features` bypass from review 170 while still allowing a
  signed package when the client supports the declared feature.

## Checks
- `cargo test -p forge-core --test spine signed_required_feature --offline`
- `cargo test -p forge-core --test required_features_vectors --offline`
