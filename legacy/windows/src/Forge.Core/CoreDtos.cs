using System.Text.Json;

namespace Forge.Core;

public sealed record ActorContext
{
    public required string Actor { get; init; }
    public required Role Role { get; init; }

    public static ActorContext Owner(string actor) => new() { Actor = actor, Role = Role.Owner };
}

public enum Role
{
    Owner,
    Maintainer,
    Editor,
    Runner,
    Viewer,
    Auditor,
    Reviewer,
}

public sealed record CoreCommand
{
    public required string RequestId { get; init; }
    public required ActorContext Actor { get; init; }
    public required string WorkspaceId { get; init; }
    public string? AppletId { get; init; }
    public required string Name { get; init; }
    public object? Payload { get; init; }

    public static CoreCommand Create(
        string requestId,
        ActorContext actor,
        string workspaceId,
        string name,
        object? payload = null,
        string? appletId = null)
    {
        return new CoreCommand
        {
            RequestId = requestId,
            Actor = actor,
            WorkspaceId = workspaceId,
            AppletId = appletId,
            Name = name,
            Payload = payload ?? new Dictionary<string, object?>(),
        };
    }
}

public sealed record CoreResponse
{
    public required string RequestId { get; init; }
    public required bool Ok { get; init; }
    public JsonElement Payload { get; init; }
    public List<string> Warnings { get; init; } = new();
    public CoreError? Error { get; init; }
}

public sealed record CoreError
{
    public required string Kind { get; init; }
    public required string Detail { get; init; }
}

public sealed record CoreEvent
{
    public required string EventId { get; init; }
    public string? AppletId { get; init; }
    public required string Kind { get; init; }
    public JsonElement Payload { get; init; }
    public required ulong CreatedAtLogical { get; init; }
}

internal sealed record EventDrainResponse
{
    public required bool Ok { get; init; }
    public List<CoreEvent> Events { get; init; } = new();
    public CoreError? Error { get; init; }
}

internal sealed record FfiErrorEnvelope
{
    public required bool Ok { get; init; }
    public CoreError? Error { get; init; }
}

