var description =
  "App Builder uses the host builderGenerate bridge to request real harness output.";

var actions = {
  status: {
    summary: "Report how App Builder generates apps.",
    args: [],
    returns: "A status string.",
    run: function () {
      return "App Builder requires the host builderGenerate bridge; no local scaffold is available.";
    },
  },
};
