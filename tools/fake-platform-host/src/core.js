export class CoreEngine {
  constructor() {
    this.stateVersions = new Map();
  }

  step(appId, event) {
    const current = this.stateVersions.get(appId) ?? 0;
    const stateVersion = current + 1;
    this.stateVersions.set(appId, stateVersion);

    return {
      stateVersion,
      actions: actionsForEvent(event),
    };
  }
}

function actionsForEvent(event) {
  switch (event?.type) {
    case "CreateTask":
      return [
        {
          type: "TaskAccepted",
          title: event.payload?.title ?? "",
          priority: event.payload?.priority ?? "medium",
        },
        { type: "Toast", message: "Task accepted" },
      ];
    case "ToggleTask":
      return [{ type: "TaskToggled", id: event.payload?.id ?? null }];
    case "TransformText":
      return [
        {
          type: "TransformText",
          text: transformText(event.payload?.text ?? "", event.payload?.mode ?? "uppercase"),
        },
      ];
    case "NetworkSnapshotReceived":
      return [{ type: "NetworkSnapshotStored", received: true }];
    default:
      return [{ type: "EventAccepted", eventType: event?.type ?? "UnknownEvent" }];
  }
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
