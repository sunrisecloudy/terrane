document.querySelector('[data-testid="run-button"]').addEventListener("click", async () => {
  await AppRuntime.call("storage.get", { key: "excessive-bridge-calls:first", defaultValue: null });
  await AppRuntime.call("storage.get", { key: "excessive-bridge-calls:second", defaultValue: null });
});
