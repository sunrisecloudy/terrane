import type { AppContext, AppResult } from "@forge/std";

type TransformInput = {
  name?: string;
  outputPath?: string;
  bytesBase64?: string;
};

export async function main(ctx: AppContext, input: TransformInput): Promise<AppResult> {
  const name = input.name ?? "sample";
  const outputPath = input.outputPath ?? "out/sample.txt";
  const bytesBase64 = input.bytesBase64 ?? "Rm9yZ2UgZmlsZSB0cmFuc2Zvcm0K";
  const write = await ctx.files.write({
    handle: "workspace_data",
    path: outputPath,
    bytes_base64: bytesBase64,
    content_type: "text/plain"
  });

  await ctx.db.insert("transforms", {
    name,
    outputPath: write.path,
    writtenBytes: write.written_bytes
  });

  ctx.ui.render({
    type: "Stack",
    testId: "file-transformer-root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "file-transformer-title", text: "File Transformer", variant: "title" },
      { type: "Text", testId: "file-transformer-output", text: `${write.path}: ${write.written_bytes} bytes` }
    ]
  });

  return { ok: true, value: { outputPath: write.path, writtenBytes: write.written_bytes } };
}
