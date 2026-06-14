# Core Command Catalog

Source of record: prd-merged/01 CR-A2 plus the committed CoreCommand envelope in forge/crates/domain/src/lib.rs. The envelope fields are request_id, actor, workspace_id, optional applet_id, name, and payload. Rows below define the payload contract that the string command name selects.

| Command | Request payload fields | Response payload | Roles | Milestone | PRD |
|---|---|---|---|---|---|
| workspace.create | name?, template?, owner_actor? | workspace_id, root_version | Owner | M0b | CR-A2 |
| workspace.open | workspace_id | path | workspace metadata, current logical clock | Owner, Maintainer, Editor, Viewer, Auditor | M0a | CR-A2 |
| workspace.export | format_version?, include_run_logs? | export artifact descriptor or bytes handle | Owner, Maintainer, Auditor | later | CR-A2, DL-24 |
| workspace.import | export artifact, conflict_policy | workspace_id, import report | Owner | later | CR-A2, DL-24 |
| applet.install | manifest, source files, signature? | applet_id, version, warnings | Owner, Maintainer | M0b | CR-A2, SC-15 |
| applet.upgrade | applet_id, manifest, source files, migration_plan? | new version, compatibility report | Owner, Maintainer | later | CR-A2 |
| applet.suspend | applet_id, reason? | status | Owner, Maintainer | later | CR-A2 |
| applet.uninstall | applet_id, retention_policy | status, retained data summary | Owner | later | CR-A2 |
| file.write | path, content, content_hash?, applet_id? | file_id/version_id | Owner, Maintainer, Editor | M0b | CR-A2 |
| file.history | path | file_id | version list | Owner, Maintainer, Editor, Viewer, Auditor | later | CR-A2 |
| file.restore_version | file_id, version_id | restored version_id | Owner, Maintainer, Editor | later | CR-A2 |
| schema.apply_change | changes[] | new registry version, warnings | Owner, Maintainer | M0a | CR-A2, DL-8 |
| schema.validate_compatibility | base_version?, proposed changes[] | ok, warnings/errors | Owner, Maintainer, Editor, Auditor | M0a | CR-A2, DL-8 |
| schema.rebuild_indexes | collection?, index_ids? | rebuild report | Owner, Maintainer | later | CR-A2, DL-5 |
| query.execute | collection, filter/order/limit or typed query | rows, cursor?, warnings | Role plus db.read capability | M0b | CR-A2, DL-15 |
| record.put | collection, id?, fields | record envelope | Role plus db.write capability | M0a | CR-A2, DL-4 |
| record.patch | collection, id, patch fields | record envelope | Role plus db.write capability | M0a | CR-A2, DL-9 |
| record.delete | collection, id | tombstone/op id | Role plus db.write capability | M0b | CR-A2 |
| record.hard_purge | collection, id, policy proof | purge report | Owner only | later | CR-A2 |
| runtime.run | applet_id, input, random_seed?, time_start? (both-or-neither; time_start ≤ i64::MAX) | run_id, result, ui patch, logs, host_call_methods | Runner, Editor, Maintainer, Owner plus caps | M0a | CR-A2, CR-8 |
| runtime.cancel | run_id | cancel status | Runner, Editor, Maintainer, Owner | M0b | CR-A2 |
| runtime.replay | run_id | run record | replayed result and diff | Auditor, Maintainer, Owner | M0a | CR-A2, CR-9 |
| runtime.replay_session | run_ids (ordered: initial run + N dispatched events) | session_fingerprint, per-event patches, final tree, replays_identically | Auditor, Maintainer, Owner | M0a | CR-A2, CR-6, CR-8 |
| runtime.get_logs | run_id, level?, cursor? | log rows | Auditor, Maintainer, Owner, Runner self | M0b | CR-A2 |
| ai.generate_patch | context, task, model_policy | proposed patch, policy findings | Maintainer, Editor with llm cap | later | CR-A2, LM-8 |
| ai.apply_patch | patch_id, human_review_id | applied files/ops | Owner, Maintainer | later | CR-A2, LM-8 |
| ai.run_fix_loop | target, failing test, bounds | attempt report | Owner, Maintainer | later | CR-A2, LM-9 |
| ai.set_context_mode | mode, scope | active context policy | Owner, Maintainer | later | CR-A2, LM-14 |
| sync.start | peer/workspace config | sync session id | Owner, Maintainer | later | CR-A2, SS-4 |
| sync.stop | session_id | stopped status | Owner, Maintainer | later | CR-A2 |
| sync.status | session_id? | state, frontier, conflicts | Owner, Maintainer, Auditor | later | CR-A2 |
| sync.invite | actor/contact, role, constraints | invite token/ref | Owner | later | CR-A2, SS-7 |
| sync.accept_invite | invite token/ref | workspace access | Invited actor | later | CR-A2 |
| permission.request_grant | applet_id, capability, reason | review request id | Editor, Maintainer, Owner | M0b | CR-A2, SC-6 |
| permission.revoke | applet_id, capability_id | revocation event | Owner, Maintainer | M0b | CR-A2, SC-6 |
| rbac.create_role | name, permissions | role id | Owner | later | CR-A2, SC-11 |
| rbac.assign_role | actor_id, role_id | assignment event | Owner | later | CR-A2, SC-11 |
| secret.store | name, value, scope, allowed_uses | secret ref | Owner, Maintainer | later | CR-A2, SC-13 |
| secret.revoke | secret_ref | revocation event | Owner, Maintainer | later | CR-A2, SC-13 |

## Commands Without A Dedicated Typed Struct Yet

Every CR-A2 command currently travels through CoreCommand.name plus serde_json payload. There are no per-command Rust request/response structs yet. The facade should add typed builders around this table while keeping the envelope stable.

## PRD Implied But Not Yet Housed

- Index definitions and rebuild reports need a storage/schema home before schema.rebuild_indexes can be implemented.
- AI review ids, patch ids, and context-mode state need an LLM subsystem table/type.
- Secret refs and allowed uses need a policy-owned type so they never degrade into plain strings.
- RBAC custom roles need persisted role/assignment rows; Role only has built-in enum variants today.
