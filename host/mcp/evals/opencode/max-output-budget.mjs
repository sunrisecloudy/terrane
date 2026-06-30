export const TerraneEvalMaxOutputBudgetPlugin = async () => {
  return {
    "chat.params": async (input, output) => {
      const envValue = Number.parseInt(
        process.env.TERRANE_OPENCODE_MAX_OUTPUT_TOKENS || "",
        10,
      );
      const modelLimit = input.model && input.model.limit
        ? input.model.limit.output
        : undefined;
      const target = Number.isFinite(envValue) && envValue > 0
        ? envValue
        : modelLimit;

      if (Number.isFinite(target) && target > 0) {
        output.maxOutputTokens = target;
      }
    },
  };
};

export default TerraneEvalMaxOutputBudgetPlugin;
