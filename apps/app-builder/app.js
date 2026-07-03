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
  var harnessEl = document.getElementById("harness");
  var promptEl = document.getElementById("prompt");
  var filesEl = document.getElementById("files");
  var statusEl = document.getElementById("status");
  var previewStatusEl = document.getElementById("preview-status");
  var previewFrame = document.getElementById("preview-frame");
  var generation = 0;

  // Translate via the host bundle when present, else fall back to English.
  // The ~15-line localize() block below is the pattern every app copies.
  function tr(key, fallback, params) {
    if (window.terrane && typeof window.terrane.t === "function") {
      var opts = { default: fallback };
      if (params) {
        for (var k in params) {
          if (Object.prototype.hasOwnProperty.call(params, k)) opts[k] = params[k];
        }
      }
      return window.terrane.t(key, opts);
    }
    return fallback;
  }

  function localize() {
    document.documentElement.dir =
      (window.terrane && window.terrane.getDir && window.terrane.getDir()) || "ltr";
    var nodes = document.querySelectorAll("[data-i18n]");
    for (var i = 0; i < nodes.length; i++) {
      nodes[i].textContent = tr(nodes[i].getAttribute("data-i18n"), nodes[i].textContent.trim());
    }
    var attrNodes = document.querySelectorAll("[data-i18n-attr]");
    for (var j = 0; j < attrNodes.length; j++) {
      attrNodes[j].getAttribute("data-i18n-attr").split(";").forEach(function (pair) {
        var kv = pair.split(":");
        if (kv.length === 2) {
          var attr = kv[0].trim();
          attrNodes[j].setAttribute(attr, tr(kv[1].trim(), attrNodes[j].getAttribute(attr)));
        }
      });
    }
    document.title = tr("app-builder.title", "App Builder");
  }

  document.getElementById("generate").addEventListener("click", generate);
  [idEl, nameEl, promptEl].forEach(function (el) {
    el.addEventListener("input", markEdited);
  });
  harnessEl.addEventListener("change", markEdited);

  function generate() {
    var ticket = ++generation;
    statusEl.textContent = tr("app-builder.generating", "Generating...");
    previewStatusEl.textContent = tr("app-builder.waiting", "Waiting");
    previewFrame.removeAttribute("src");
    if (typeof terrane.builderGenerate !== "function") {
      statusEl.textContent = tr("app-builder.failedNoBridge", "Failed: builder bridge unavailable");
      previewStatusEl.textContent = tr("app-builder.unavailable", "Unavailable");
      renderFiles([]);
      return;
    }
    terrane.builderGenerate({
      id: slug(idEl.value) || "my-app",
      name: title(nameEl.value, "My App"),
      prompt: title(promptEl.value, "A local-first Terrane app."),
      harness: harnessEl.value || "codex",
    })
      .then(function (result) {
        if (ticket !== generation) return null;
        var files = result.files || [];
        renderFiles(files);
        if (result.status === "failed") {
          statusEl.textContent = tr("app-builder.failedReason", "Failed: {reason}", {
            reason: result.error || tr("app-builder.builderFailed", "builder failed"),
          });
          previewStatusEl.textContent = tr("app-builder.failed", "Failed");
          return null;
        }
        statusEl.textContent = tr("app-builder.fileCount", "{count} files", { count: files.length });
        return renderPreview(ticket, files);
      })
      .catch(function (error) {
        if (ticket !== generation) return;
        statusEl.textContent = tr("app-builder.failedReason", "Failed: {reason}", { reason: error.message });
        previewStatusEl.textContent = tr("app-builder.failed", "Failed");
      });
  }

  function markEdited() {
    statusEl.textContent = tr("app-builder.edited", "Edited");
    previewStatusEl.textContent = tr("app-builder.edited", "Edited");
  }

  function renderPreview(ticket, files) {
    if (!files.length) {
      previewStatusEl.textContent = tr("app-builder.noFiles", "No files");
      return Promise.resolve();
    }
    if (typeof terrane.preview !== "function") {
      previewStatusEl.textContent = tr("app-builder.unavailable", "Unavailable");
      return Promise.resolve();
    }
    previewStatusEl.textContent = tr("app-builder.loading", "Loading...");
    return terrane.preview(files)
      .then(function (preview) {
        if (ticket !== generation) return;
        if (!preview || !preview.frameUrl) {
          throw new Error(tr("app-builder.missingFrameUrl", "missing preview frameUrl"));
        }
        previewFrame.src = preview.frameUrl;
        previewStatusEl.textContent = tr("app-builder.ready", "Ready");
      })
      .catch(function (error) {
        if (ticket !== generation) return;
        previewStatusEl.textContent = tr("app-builder.failedReason", "Failed: {reason}", { reason: error.message });
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

  localize();
  // The host pushes the locale bundle shortly after load; re-localize then,
  // and on any later language change.
  if (window.terrane && typeof window.terrane.onMessages === "function") {
    window.terrane.onMessages(localize);
  }
})();
