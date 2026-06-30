# Login, Logout, And Sessions

UI behavior for these states is expanded in `10-login-logout-ui.md`.

## Local Mode

Local Terrane must remain useful without login.

On first local run, Terrane seeds:

```text
org:local
user:local-owner
membership: local-owner is owner of org:local
```

This is not a SaaS account. It is a local authority record for one
`TERRANE_HOME`.

## Local Login Step

Local login is mostly "unlock current local authority" rather than "authenticate
to the cloud."

Initial local v1 can be:

```text
1. User opens local Terrane.
2. Host creates or loads org:local and user:local-owner.
3. Host creates an in-memory session:
   subject = user:local-owner
   org = local
   source = local
4. Admin UI can use this session to grant/revoke local app and agent permissions.
```

Later local hardening can add OS account binding, passkey, Keychain, Touch ID, or
device unlock before sensitive admin actions.

## Local Logout Step

Local logout ends local admin authority for the running host session. It should
not delete apps or data.

```text
1. User clicks logout / lock local admin.
2. Host drops the in-memory admin session.
3. App runtimes continue to run only with already-granted resource policy.
4. Admin mutations require unlocking/login again.
```

For v1, logout can be a host UI state. Later it can revoke local session tokens
or clear platform credential-cache entries.

## Premium Login Step

Premium login is real account authentication and device authorization.

```text
1. User chooses "Sign in to Terrane Premium".
2. Platform-owned client opens Premium auth flow.
3. Premium authenticates user.
4. Premium binds session to user and device.
5. Client receives platform tokens stored in OS credential storage.
6. Client fetches org memberships, entitlements, device policy, and policy
   snapshot metadata.
7. Local Terrane imports only non-secret policy snapshots into reserved KV.
```

Generated apps never receive Premium tokens.

## Premium Logout Step

Premium logout removes cloud authority and stops Premium sync/control-plane
operations. It does not remove local app data by default.

```text
1. User clicks logout from Premium.
2. Client calls Premium session revoke when online.
3. Client clears local Premium tokens from OS credential storage.
4. Local admin session falls back to org:local or locked state.
5. Premium policy snapshots remain cached only if their offline validity allows
   it; otherwise cloud-managed grants are treated as expired.
6. Generated apps still cannot access SaaS credentials.
```

## Session Types

```text
local_admin_session
  org = local
  subject = user:local-owner
  stored = memory, later OS-protected

premium_user_session
  org = selected Premium org
  subject = user:<premium-user-id>
  stored = OS credential storage

agent_session
  subject = ai_agent
  owner user = user subject
  authority = delegated subset
```

## Sensitive Actions

These should require an active admin-capable session:

- grant resource permission;
- revoke resource permission;
- assign role;
- invite/remove member;
- register/revoke AI agent;
- approve app permission request;
- change org policy;
- export audit/policy data.

Premium can later require recent auth or MFA for selected mutations.
