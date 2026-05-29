document.querySelector('[data-testid="run-button"]').addEventListener("click", () => {
  AppRuntime.call("storage.get", { key: "notes-lite:notes", defaultValue: [] });
});
