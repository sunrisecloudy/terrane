export class CoreEngine {
  constructor() {
    this.stateVersions = new Map();
  }

  step(appId, event) {
    const validationError = validateCoreEvent(event);
    if (validationError) {
      return {
        ok: false,
        error: validationError,
        actions: [],
      };
    }

    const current = this.stateVersions.get(appId) ?? 0;
    const stateVersion = current + 1;
    this.stateVersions.set(appId, stateVersion);

    return {
      ok: true,
      stateVersion,
      actions: actionsForEvent(event),
    };
  }

  snapshot(appId = null) {
    if (appId) {
      return { appId, stateVersion: this.stateVersions.get(appId) ?? 0 };
    }
    return {
      apps: [...this.stateVersions.entries()]
        .map(([id, stateVersion]) => ({ appId: id, stateVersion }))
        .sort((a, b) => a.appId.localeCompare(b.appId)),
    };
  }

  replay(appId, events = []) {
    const replayCore = new CoreEngine();
    return events.map((event, index) => ({
      index,
      event,
      result: replayCore.step(appId, event),
    }));
  }
}

function validateCoreEvent(event) {
  if (event === undefined) {
    return { code: "invalid_event", message: "core.step input requires event" };
  }
  if (!event || typeof event !== "object" || Array.isArray(event)) {
    return { code: "invalid_event", message: "event must be an object" };
  }
  if (!("type" in event)) {
    return { code: "invalid_event", message: "event.type is required" };
  }
  if (typeof event.type !== "string") {
    return { code: "invalid_event", message: "event.type must be a string" };
  }
  return null;
}

function actionsForEvent(event) {
  switch (event?.type) {
    case "CreateTask":
      return [
        {
          type: "Toast",
          message: `Task accepted: ${payloadString(event.payload, "title") ?? "task"}`,
          level: "success",
        },
        { type: "Log", message: "CreateTask handled" },
      ];
    case "NetworkSnapshotReceived":
      return [{ type: "RenderHint", hint: "network-snapshot-received" }];
    case "TransformText":
      return [
        {
          type: "TransformText",
          text: transformText(payloadString(event.payload, "text") ?? "", payloadString(event.payload, "mode") ?? "uppercase"),
        },
      ];
    default:
      return [
        {
          type: "Log",
          message: `Unhandled event: ${event.type}`,
        },
      ];
  }
}

function payloadString(payload, field) {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) return null;
  return typeof payload[field] === "string" ? payload[field] : null;
}

function transformText(text, mode) {
  if (mode === "lowercase") return text.toLowerCase();
  if (mode === "reverse-lines") return text.split(/\r?\n/).reverse().join("\n");
  if (mode === "word-count") {
    const words = text.trim() ? text.trim().split(/\s+/).length : 0;
    const lines = text ? text.split(/\r?\n/).length : 0;
    return `Words: ${words}\nLines: ${lines}\nCharacters: ${text.length}`;
  }
  return text.toUpperCase();
}
