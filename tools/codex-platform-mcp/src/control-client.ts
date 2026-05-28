import type { ControlResponse, ToolName } from "./tool-contract.js";

export class ControlClient {
  constructor(
    private readonly baseUrl: string,
    private readonly token: string,
  ) {}

  async command<T = unknown>(tool: ToolName, args: unknown): Promise<ControlResponse<T>> {
    const response = await fetch(`${this.baseUrl}/command`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "Authorization": `Bearer ${this.token}`,
      },
      body: JSON.stringify({ tool, args }),
    });

    const body = await response.json() as ControlResponse<T>;

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
