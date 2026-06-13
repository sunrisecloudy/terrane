using System.Text.Json;

namespace Forge.Core;

public sealed class CoreClient : IDisposable
{
    private readonly object gate = new();
    private IntPtr handle;
    private bool disposed;

    private CoreClient(IntPtr handle)
    {
        if (handle == IntPtr.Zero)
        {
            throw NativeMethods.CreateExceptionFromLastError("Opening forge core failed.");
        }

        this.handle = handle;
    }

    ~CoreClient()
    {
        Dispose(false);
    }

    public static CoreClient Open(string path, string workspaceId)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path);
        ArgumentException.ThrowIfNullOrWhiteSpace(workspaceId);
        return new CoreClient(NativeMethods.ForgeCoreOpen(path, workspaceId));
    }

    public static CoreClient OpenInMemory(string workspaceId)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(workspaceId);
        return new CoreClient(NativeMethods.ForgeCoreOpenInMemory(workspaceId));
    }

    public CoreResponse Handle(CoreCommand command)
    {
        ArgumentNullException.ThrowIfNull(command);
        var commandJson = JsonSerializer.Serialize(command, CoreJson.Options);
        return HandleJson(commandJson);
    }

    public CoreResponse HandleJson(string commandJson)
    {
        ArgumentNullException.ThrowIfNull(commandJson);
        var responseJson = InvokeString(currentHandle =>
            NativeMethods.ForgeCoreHandleCommand(currentHandle, commandJson));
        return JsonSerializer.Deserialize<CoreResponse>(responseJson, CoreJson.Options)
            ?? throw new ForgeCoreException("Native forge call returned an empty CoreResponse.");
    }

    public IReadOnlyList<CoreEvent> DrainEvents()
    {
        var drainJson = InvokeString(NativeMethods.ForgeCoreDrainEvents);
        var drain = JsonSerializer.Deserialize<EventDrainResponse>(drainJson, CoreJson.Options)
            ?? throw new ForgeCoreException("Native forge call returned an empty event drain response.");
        if (!drain.Ok)
        {
            throw new ForgeCoreException(
                drain.Error is null ? "Draining events failed." : $"{drain.Error.Kind}: {drain.Error.Detail}",
                drain.Error,
                drainJson);
        }

        return drain.Events;
    }

    public void Dispose()
    {
        Dispose(true);
        GC.SuppressFinalize(this);
    }

    private string InvokeString(Func<IntPtr, IntPtr> invoke)
    {
        lock (gate)
        {
            ThrowIfDisposed();
            return NativeMethods.TakeUtf8String(invoke(handle));
        }
    }

    private void Dispose(bool disposing)
    {
        lock (gate)
        {
            if (disposed)
            {
                return;
            }

            if (handle != IntPtr.Zero)
            {
                NativeMethods.ForgeCoreClose(handle);
                handle = IntPtr.Zero;
            }

            disposed = true;
        }
    }

    private void ThrowIfDisposed()
    {
        ObjectDisposedException.ThrowIf(disposed, this);
    }
}

