---
name: Zig Server API Smoke
base_url: http://127.0.0.1:18088
timeout: 10s
control_token: server-smoke-token
---

# Zig Server API Smoke

Run this suite against a dev Zig server started with
`NATIVE_AI_SERVER_CONTROL_TOKEN=server-smoke-token`.

## Health

```http
GET /health
```

```expect
status == 200
body.ok == true
body.version == "0.1.0"
body.target == "zig-server"
```

## Core Step

```http
POST /core/step
Content-Type: application/json
```

```json
{
  "app": "task-workbench",
  "event": {
    "type": "CreateTask",
    "payload": {
      "title": "server smoke task"
    }
  },
  "context": {
    "platform": "server",
    "runtimeVersion": "0.1.0"
  }
}
```

```expect
status == 200
body.ok == true
body.stateVersion == 1
body.actions | length(@) > 0
body.actions[0].type == "Toast"
```

## Example Catalog

```http
GET /webapps/examples.json
```

```expect
status == 200
body.ok == true
body.examples | length(@) == 5
body.examples[0].id == "api-dashboard"
body.examples[4].id == "task-workbench"
```

## Package Validation Rejects Missing Manifest

```http
POST /webapps/validate
Content-Type: application/json
```

```json
{}
```

```expect
status == 200
body.ok == false
body.status == "rejected"
body.errors[0] == "missing_manifest"
```

## Bridge Requires Channel-Derived App Id

```http
POST /bridge
Content-Type: application/json
```

```json
{
  "id": "req_missing_channel",
  "method": "runtime.capabilities",
  "params": {}
}
```

```expect
status == 200
body.ok == false
body.error.code == "bridge.unauthorized_channel"
```

## Control Command Requires Token

```http
POST /control/command
Content-Type: application/json
```

```json
{
  "tool": "platform.health",
  "args": {}
}
```

```expect
status == 401
body.ok == false
body.error.code == "control_auth_required"
```

## Platform Health Control Command

```http
POST /control/command
Content-Type: application/json
X-Platform-Control-Token: {{control_token}}
```

```json
{
  "tool": "platform.health",
  "args": {}
}
```

```expect
status == 200
body.ok == true
body.result.name == "zig-server"
body.result.version == "0.1.0"
body.result.db == "sqlite"
body.result.targets[0] == "zig-server"
```

## Platform Target List Control Command

```http
POST /control/command
Content-Type: application/json
X-Platform-Control-Token: {{control_token}}
```

```json
{
  "tool": "platform.list_targets",
  "args": {}
}
```

```expect
status == 200
body.ok == true
body.result.targets | length(@) == 1
body.result.targets[0].id == "server"
body.result.targets[0].platform == "server"
body.result.targets[0].status == "available"
body.result.targets[0].runtimeVersion == "0.1.0"
```

## Runtime Capabilities Control Command

```http
POST /control/command
Content-Type: application/json
X-Platform-Control-Token: {{control_token}}
```

```json
{
  "tool": "runtime.capabilities",
  "args": {}
}
```

```expect
status == 200
body.ok == true
body.result.runtimeVersion == "0.1.0"
body.result.platform == "server"
body.result.target == "zig-server"
body.result.features."core.step" == true
body.result.features."runtime.capabilities" == true
body.result.features."storage.get" == true
```

## DB Snapshot Control Endpoint

```http
POST /db/snapshot
Content-Type: application/json
X-Platform-Control-Token: {{control_token}}
```

```json
{}
```

```expect
status == 200
body.ok == true
body.result.apps | type(@) == "array"
body.result.app_versions | type(@) == "array"
body.result.bridge_calls | type(@) == "array"
body.result.core_events | type(@) == "array"
body.result.control_commands | type(@) == "array"
```
