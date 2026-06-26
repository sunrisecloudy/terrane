export class ControlClient {
  constructor(baseUrl, token) {
    this.baseUrl = baseUrl;
    this.token = token;
  }

  async command(tool, args) {
    const response = await fetch(`${this.baseUrl}/control/command`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-platform-control-token": this.token,
      },
      body: JSON.stringify({ tool, args: args ?? {} }),
    });

    const body = await response.json();
    if (!response.ok && body.ok !== false) {
      return {
        ok: false,
        error: {
          code: "control.http_error",
          message: `Control plane returned HTTP ${response.status}`,
          details: body,
        },
      };
    }
    return body;
  }
}
