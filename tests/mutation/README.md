# Mutation Security and Validator Tests

Each fixture intentionally breaks one rule. The validator or runtime must reject it with a precise error code.

Required mutation categories:

- missing required manifest field;
- invalid permission;
- forbidden bridge method;
- direct `fetch`;
- `eval` or `new Function`;
- remote script or stylesheet;
- invalid storage prefix;
- invalid network policy;
- oversized package/file;
- post-signature tampering;
- missing migration after dataVersion change;
- resource budget exceeded.
