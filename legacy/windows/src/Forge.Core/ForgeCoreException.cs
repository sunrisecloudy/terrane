namespace Forge.Core;

public sealed class ForgeCoreException : Exception
{
    public ForgeCoreException(string message, CoreError? error = null, string? nativeJson = null)
        : base(message)
    {
        Error = error;
        NativeJson = nativeJson;
    }

    public CoreError? Error { get; }

    public string? NativeJson { get; }
}

