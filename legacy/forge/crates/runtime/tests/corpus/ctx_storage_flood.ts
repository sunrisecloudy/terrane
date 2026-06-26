type FloodContext = {
  storage: {
    get(key: string): Promise<string | null>;
  };
};

export async function main(ctx: FloodContext, _input: unknown): Promise<unknown> {
  while (true) {
    await ctx.storage.get("flood");
  }
}
