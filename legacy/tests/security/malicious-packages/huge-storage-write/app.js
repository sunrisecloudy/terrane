document.querySelector('[data-testid="run-button"]').addEventListener("click", () => {
  AppRuntime.call("storage.set", { key: "huge-storage-write:blob", value: "this value is deliberately too large" });
});
