import type { AppContext, AppResult } from "@forge/std";

type ApiInput = {
  path?: string;
};

export async function main(ctx: AppContext, input: ApiInput): Promise<AppResult> {
  const path = input.path ?? "weather";
  const url = `https://api.example.com/public/${path}`;
  const net = (ctx as any).net;
  const response = await net.fetch({
    method: "GET",
    url,
    response_content_type: "application/json"
  });

  await ctx.db.insert("requests", {
    url,
    status: response.status,
    contentType: response.content_type ?? "application/json"
  });
  const requests = await ctx.db.list("requests");

  ctx.ui.render({
    type: "Stack",
    testId: "api-dashboard-root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "api-dashboard-title", text: "API Dashboard", variant: "title" },
      {
        type: "List",
        testId: "api-dashboard-requests",
        items: requests.map((request: any) => ({
          type: "Text",
          testId: `request-${request.status ?? request.fields?.status}`,
          text: `${request.status ?? request.fields?.status} ${request.url ?? request.fields?.url}`
        }))
      }
    ]
  });

  return { ok: true, value: { status: response.status, count: requests.length } };
}
