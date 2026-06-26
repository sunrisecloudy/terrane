using System.Runtime.InteropServices;
using System.Text.Json;

namespace Forge.Core;

internal static partial class NativeMethods
{
    private const string LibraryName = "forge_ffi";

    [LibraryImport(LibraryName, EntryPoint = "forge_core_open", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr ForgeCoreOpen(string path, string workspaceId);

    [LibraryImport(LibraryName, EntryPoint = "forge_core_open_in_memory", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr ForgeCoreOpenInMemory(string workspaceId);

    [LibraryImport(LibraryName, EntryPoint = "forge_core_handle_command", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr ForgeCoreHandleCommand(IntPtr handle, string commandJson);

    [LibraryImport(LibraryName, EntryPoint = "forge_core_drain_events")]
    internal static partial IntPtr ForgeCoreDrainEvents(IntPtr handle);

    [LibraryImport(LibraryName, EntryPoint = "forge_core_last_error")]
    internal static partial IntPtr ForgeCoreLastError();

    [LibraryImport(LibraryName, EntryPoint = "forge_core_close")]
    internal static partial void ForgeCoreClose(IntPtr handle);

    [LibraryImport(LibraryName, EntryPoint = "forge_string_free")]
    internal static partial void ForgeStringFree(IntPtr value);

    internal static string TakeUtf8String(IntPtr value)
    {
        if (value == IntPtr.Zero)
        {
            throw CreateExceptionFromLastError("Native forge call returned a null string pointer.");
        }

        try
        {
            return Marshal.PtrToStringUTF8(value) ?? string.Empty;
        }
        finally
        {
            ForgeStringFree(value);
        }
    }

    internal static ForgeCoreException CreateExceptionFromLastError(string fallbackMessage)
    {
        var lastErrorJson = TryTakeLastErrorJson();
        if (lastErrorJson is null)
        {
            return new ForgeCoreException(fallbackMessage);
        }

        try
        {
            var envelope = JsonSerializer.Deserialize<FfiErrorEnvelope>(lastErrorJson, CoreJson.Options);
            var error = envelope?.Error;
            var message = error is null ? fallbackMessage : $"{error.Kind}: {error.Detail}";
            return new ForgeCoreException(message, error, lastErrorJson);
        }
        catch (JsonException)
        {
            return new ForgeCoreException(fallbackMessage, nativeJson: lastErrorJson);
        }
    }

    private static string? TryTakeLastErrorJson()
    {
        var value = ForgeCoreLastError();
        if (value == IntPtr.Zero)
        {
            return null;
        }

        try
        {
            return Marshal.PtrToStringUTF8(value);
        }
        finally
        {
            ForgeStringFree(value);
        }
    }
}

