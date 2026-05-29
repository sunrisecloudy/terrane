# Mutation Security and Validator Tests

Each fixture intentionally breaks one rule. The validator or runtime must reject it with a precise error code.

Required mutation categories:

- missing required manifest field;
- invalid permission;
- forbidden bridge method;
- direct network APIs (`fetch`, `XMLHttpRequest`, `WebSocket`, `EventSource`);
- direct browser persistence APIs (`localStorage`, `IndexedDB`, cookies);
- direct sandbox escape APIs (`window.parent`, `window.top`, `window.opener`);
- service worker registration;
- `eval` or `new Function`;
- inline styles or CSP that allows inline styles;
- missing, duplicate, or non-plain `app.js` script tags;
- missing, alternate, duplicate, or non-plain `styles.css` stylesheet links;
- remote script or stylesheet;
- invalid storage prefix;
- invalid network policy;
- oversized package/file;
- post-signature tampering;
- missing migration after dataVersion change;
- resource budget exceeded.
