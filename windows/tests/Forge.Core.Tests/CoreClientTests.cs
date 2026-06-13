using Forge.Core;
using Xunit;

namespace Forge.Core.Tests;

public sealed class CoreClientTests
{
    private const string WorkspaceId = "ws-csharp";
    private const string DemoAppletId = "app_demo";

    private const string DemoTs = """
        export async function main(ctx: any, input: any): Promise<any> {
            const title: string = input && input.title ? input.title : "Ship W0";
            const id = await ctx.db.insert("tasks", { title: title, done: false });
            await ctx.storage.set("app/last", { id: id });
            ctx.log("rendered task " + id);
            await ctx.ui.render({
                type: "Stack",
                direction: "v",
                children: [
                    { type: "Text", text: "Tasks" },
                    { type: "List", items: [ { type: "Text", text: title } ] }
                ]
            });
            return { ok: true, value: { id: id } };
        }
        """;

    [Fact]
    public void WorkspaceOpenReturnsPayloadFromRealCore()
    {
        using var client = CoreClient.OpenInMemory(WorkspaceId);

        var response = client.Handle(CoreCommand.Create(
            "req-open",
            ActorContext.Owner("dev"),
            WorkspaceId,
            "workspace.open"));

        Assert.True(response.Ok, response.Error?.Detail);
        Assert.Equal(WorkspaceId, response.Payload.GetProperty("workspace_id").GetString());
    }

    [Fact]
    public void InstallRunAndDrainEventsCrossTheBoundary()
    {
        using var client = CoreClient.OpenInMemory(WorkspaceId);

        var install = client.Handle(CoreCommand.Create(
            "req-install",
            ActorContext.Owner("dev"),
            WorkspaceId,
            "applet.install",
            new
            {
                manifest = DemoManifest(),
                sources = new Dictionary<string, string> { ["src/main.ts"] = DemoTs },
            },
            DemoAppletId));
        Assert.True(install.Ok, install.Error?.Detail);

        var run = client.Handle(CoreCommand.Create(
            "req-run",
            ActorContext.Owner("dev"),
            WorkspaceId,
            "runtime.run",
            new { input = new { title = "Buy milk" } },
            DemoAppletId));
        Assert.True(run.Ok, run.Error?.Detail);
        Assert.True(run.Payload.GetProperty("ok").GetBoolean());
        Assert.Equal("tasks/1", run.Payload.GetProperty("result").GetProperty("value").GetProperty("id").GetString());

        var events = client.DrainEvents();
        Assert.Contains(events, e => e.Kind == "applet.installed");
        Assert.Contains(events, e => e.Kind == "run.started");
        Assert.Contains(events, e => e.Kind == "ui.patch");
        Assert.Contains(events, e => e.Kind == "run.completed");

        var uiPatch = Assert.Single(events, e => e.Kind == "ui.patch");
        Assert.Contains("Buy milk", uiPatch.Payload.GetProperty("tree").GetRawText(), StringComparison.Ordinal);
    }

    [Fact]
    public void MalformedJsonReturnsStructuredCoreResponse()
    {
        using var client = CoreClient.OpenInMemory(WorkspaceId);

        var response = client.HandleJson("{");

        Assert.False(response.Ok);
        Assert.Equal("ValidationError", response.Error?.Kind);
    }

    [Fact]
    public void ForbiddenRoleCommandReturnsStructuredError()
    {
        using var client = CoreClient.OpenInMemory(WorkspaceId);

        var response = client.Handle(CoreCommand.Create(
            "req-denied",
            new ActorContext { Actor = "viewer", Role = Role.Viewer },
            WorkspaceId,
            "runtime.run",
            new { input = new { } },
            DemoAppletId));

        Assert.False(response.Ok);
        Assert.Equal("PermissionDenied", response.Error?.Kind);
    }

    [Fact]
    public void DisposeIsIdempotentAndPreventsFurtherUse()
    {
        var client = CoreClient.OpenInMemory(WorkspaceId);

        client.Dispose();
        client.Dispose();

        Assert.Throws<ObjectDisposedException>(() => client.Handle(CoreCommand.Create(
            "req-after-dispose",
            ActorContext.Owner("dev"),
            WorkspaceId,
            "workspace.open")));
    }

    private static object DemoManifest()
    {
        return new
        {
            entrypoint = "src/main.ts",
            min_api = "forge-api@0.1",
            deterministic = true,
            capabilities = new
            {
                storage = new { read = new[] { "app/*" }, write = new[] { "app/*" } },
                db = new { read = new[] { "tasks" }, write = new[] { "tasks" } },
                ui = true,
            },
            limits = new
            {
                wall_ms = 3000,
                fuel = 10000000,
                memory_bytes = 67108864,
                max_host_calls = 10000,
                storage_bytes = 10485760,
                log_bytes = 262144,
            },
        };
    }
}
