# Zig Server API Smoke

```setup
save "http://127.0.0.1:18088" as base_url
```

## Health

```curl
curl {{base_url}}/health
```

```then
status is 200
ok is true
version is "0.1.0"
target is "zig-server"
```

## Core Step

```curl
curl -X POST {{base_url}}/core/step \
  -H "Content-Type: application/json" \
  -d '{"app":"task-workbench","event":{"type":"CreateTask","payload":{"title":"server smoke task"}},"context":{"platform":"server","runtimeVersion":"0.1.0"}}'
```

```then
status is 200
ok is true
stateVersion is 1
actions is type array
actions has length > 0
actions[0].type is "Toast"
```
