var description =
  "Developer preview for local Apple Photos intake, classification, and route recommendations.";

var actions = {
  status: {
    summary: "Report Visual Intake developer-preview status.",
    args: [],
    returns: "A status string.",
    run: function () {
      return "Visual Intake uses the trusted macOS PhotoKit and Vision bridge.";
    },
  },
};
