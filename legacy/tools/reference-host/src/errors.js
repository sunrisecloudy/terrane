export class PlatformError extends Error {
  constructor(code, message, details = {}) {
    super(message);
    this.name = "PlatformError";
    this.code = code;
    this.details = details;
  }
}

export function errorBody(error) {
  if (error instanceof PlatformError) {
    return {
      code: error.code,
      message: error.message,
      details: error.details ?? {},
    };
  }

  return {
    code: "internal_error",
    message: error instanceof Error ? error.message : String(error),
    details: {},
  };
}

export function bridgeError(id, error) {
  return {
    id,
    ok: false,
    error: errorBody(error),
  };
}

export function bridgeOk(id, result) {
  return {
    id,
    ok: true,
    result,
  };
}
