# Capability Grammar

Source of record: prd-merged/07 SC-8 plus current forge-domain Manifest.capabilities. Full v1 capabilities are action + resource + constraints. No wildcard network domains are allowed; every grant is human-reviewed before it can become active.

## Canonical Shape

A full grant should be represented as JSON like:

~~~json
{
  "id": "cap_...",
  "namespace": "db",
  "action": "read",
  "resource": "collection:tasks",
  "constraints": {},
  "grantedBy": "actor_owner",
  "grantedAtLogical": 42
}
~~~

| Namespace | Actions | Resource shape | Constraint shape | Example grant JSON | M0a status |
|---|---|---|---|---|---|
| db | read/write | collection or query scope | collection, field mask, max rows | {"namespace":"db","action":"read","resource":"collection:tasks","constraints":{"limit":1000}} | Partial M0a via Manifest.capabilities.db.read/write |
| storage | read/write/delete/list | applet namespace key prefix | prefix, byte budget | {"namespace":"storage","action":"write","resource":"kv:notes/*","constraints":{"maxBytes":1048576}} | Partial M0a via Manifest.capabilities.storage.read/write |
| ui | render/event | applet UI tree | component set, event names | {"namespace":"ui","action":"render","resource":"tree","constraints":{"components":["Stack","Text","Button"]}} | M0a boolean ui cap |
| net | request | scheme/host/path/method | headers, body/response bytes, content type, timeout, DNS pin | {"namespace":"net","action":"request","resource":"https://api.example.com/public/*","constraints":{"methods":["GET"],"maxResponseBytes":1048576}} | Planned |
| llm | generate/propose_patch | model or local provider | context mode, token budget, tool ban | {"namespace":"llm","action":"generate","resource":"local:model/default","constraints":{"maxTokens":2048}} | Planned |
| schedule | create/cancel/list | timer/job id | earliest/latest, repeat policy | {"namespace":"schedule","action":"create","resource":"job:daily-summary","constraints":{"minIntervalSeconds":3600}} | Planned |
| secrets | store/use/revoke | secret ref only | allowed net destinations, header/param target | {"namespace":"secrets","action":"use","resource":"secret:weather_api","constraints":{"netHosts":["api.example.com"],"injectInto":"header"}} | Planned |
| files | read/write/history | workspace file path | path prefix, content type, max bytes | {"namespace":"files","action":"write","resource":"workspace:/applets/notes/*","constraints":{"maxBytes":65536}} | Planned |
| time | now | deterministic time source | recorded/seeded mode | {"namespace":"time","action":"now","resource":"clock:deterministic","constraints":{}} | M0a host seam |
| random | next | deterministic random source | seeded/recorded stream | {"namespace":"random","action":"next","resource":"rng:deterministic","constraints":{}} | M0a host seam |
| platform | target-specific actions | named platform feature | must be explicit per feature | {"namespace":"platform","action":"notify","resource":"desktop:notification","constraints":{"urgency":"normal"}} | Planned |

## Current Code Delta

The committed Manifest.capabilities type is intentionally smaller than SC-8: storage and db are read/write string lists, ui is a bool, and net/llm/schedule/secrets/files/platform grants are not modeled yet. The policy facade should accept the richer grammar and downcast only the M0a subset into the current Manifest shape until domain grows the full type.
