(function () {
  var bridge = window.terrane || {};
  var terrane = {
    preview: typeof bridge.preview === "function" ? bridge.preview.bind(bridge) : null,
    builderGenerate: typeof bridge.builderGenerate === "function"
      ? bridge.builderGenerate.bind(bridge)
      : null,
  };
  var idEl = document.getElementById("app-id");
  var nameEl = document.getElementById("app-name");
  var promptEl = document.getElementById("prompt");
  var filesEl = document.getElementById("files");
  var statusEl = document.getElementById("status");
  var previewStatusEl = document.getElementById("preview-status");
  var previewFrame = document.getElementById("preview-frame");
  var generation = 0;

  document.getElementById("generate").addEventListener("click", generate);
  [idEl, nameEl, promptEl].forEach(function (el) {
    el.addEventListener("input", function () {
      statusEl.textContent = "Edited";
      previewStatusEl.textContent = "Edited";
    });
  });

  generate();

  function generate() {
    var ticket = ++generation;
    statusEl.textContent = "Generating...";
    previewStatusEl.textContent = "Waiting";
    previewFrame.removeAttribute("src");
    if (typeof terrane.builderGenerate !== "function") {
      statusEl.textContent = "Failed: builder bridge unavailable";
      previewStatusEl.textContent = "Unavailable";
      renderFiles([]);
      return;
    }
    terrane.builderGenerate({
      id: slug(idEl.value) || "my-app",
      name: title(nameEl.value, "My App"),
      prompt: title(promptEl.value, "A local-first Terrane app."),
      agent: "codex",
    })
      .then(function (result) {
        if (ticket !== generation) return null;
        var files = result.files || [];
        renderFiles(files);
        if (result.status === "failed") {
          statusEl.textContent = "Failed: " + (result.error || "builder failed");
          previewStatusEl.textContent = "Failed";
          return null;
        }
        statusEl.textContent = files.length + " files";
        return renderPreview(ticket, files);
      })
      .catch(function (error) {
        if (ticket !== generation) return;
        statusEl.textContent = "Failed: " + error.message;
        previewStatusEl.textContent = "Failed";
      });
  }

  function renderPreview(ticket, files) {
    if (!files.length) {
      previewStatusEl.textContent = "No files";
      return Promise.resolve();
    }
    if (typeof terrane.preview !== "function") {
      previewStatusEl.textContent = "Unavailable";
      return Promise.resolve();
    }
    previewStatusEl.textContent = "Loading...";
    return terrane.preview(files)
      .then(function (preview) {
        if (ticket !== generation) return;
        if (!preview || !preview.frameUrl) {
          throw new Error("missing preview frameUrl");
        }
        previewFrame.src = preview.frameUrl;
        previewStatusEl.textContent = "Ready";
      })
      .catch(function (error) {
        if (ticket !== generation) return;
        previewStatusEl.textContent = "Failed: " + error.message;
      });
  }

  function renderFiles(files) {
    filesEl.textContent = "";
    files.forEach(function (file) {
      var row = document.createElement("article");
      row.className = "file";
      var name = document.createElement("div");
      name.className = "file-name";
      name.textContent = file.path;
      var pre = document.createElement("pre");
      pre.textContent = file.content;
      row.appendChild(name);
      row.appendChild(pre);
      filesEl.appendChild(row);
    });
  }

  function slug(input) {
    return String(input || "")
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, "-")
      .replace(/^-+|-+$/g, "");
  }

  function title(input, fallback) {
    var text = String(input || "").trim();
    return text || fallback;
  }
})();
