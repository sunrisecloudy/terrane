# Terrane Premium session client

`TerranePremiumSession` is the host-owned Apple-platform boundary for Premium
identity and authenticated API calls. It supports iOS 16+ and macOS 13+.

- Access tokens exist only in the `PremiumSessionClient` actor.
- Refresh tokens are stored as device-only Keychain generic passwords.
- Generated app `WKWebView`s and `TerraneBridge` do not import this package and
  receive neither token type.
- A missing account never blocks local Terrane operation.
- Refreshes are single-flight and authenticated requests retry once after a
  `401`.
- Provider-linking and account-deletion endpoints remain server-evolvable
  through host lifecycle wrappers; successful deletion clears local credentials.
- Responses follow the Premium `{ "ok": true, "result": ... }` envelope;
  session tokens are decoded from `result.session`.

Host UI code should translate Sign in with Apple or Google SDK results into
`PremiumAppleCredential` / `PremiumGoogleCredential`, then call the matching
exchange method. Authenticated Premium features call `send`; they must not build
their own bearer headers.
