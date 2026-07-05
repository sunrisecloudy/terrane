ctx.resource.document.create(
  "daily-plan",
  "Daily Plan",
  "## Today\n- Ship the capability docs",
  JSON.stringify({ contentType: "text/markdown", tags: ["planning"] })
);
