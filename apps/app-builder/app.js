(function () {
  var fallback = {
    invoke: function (_verb, id, name, description) {
      return Promise.resolve(JSON.stringify(localScaffold(id, name, description)));
    },
    preview: function () {
      return Promise.reject(new Error("Preview bridge unavailable"));
    }
  };
  var bridge = window.terrane || {};
  var terrane = {
    invoke: typeof bridge.invoke === "function" ? bridge.invoke.bind(bridge) : fallback.invoke,
    preview: typeof bridge.preview === "function" ? bridge.preview.bind(bridge) : fallback.preview
  };
  var idEl = document.getElementById("app-id");
  var nameEl = document.getElementById("app-name");
  var descEl = document.getElementById("description");
  var filesEl = document.getElementById("files");
  var statusEl = document.getElementById("status");
  var previewStatusEl = document.getElementById("preview-status");
  var previewFrame = document.getElementById("preview-frame");
  var generation = 0;

  document.getElementById("generate").addEventListener("click", generate);
  [idEl, nameEl, descEl].forEach(function (el) {
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
    terrane.invoke("scaffold", idEl.value, nameEl.value, descEl.value)
      .then(function (json) {
        if (ticket !== generation) return null;
        var result = JSON.parse(json || "{}");
        var files = result.files || [];
        renderFiles(files);
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
        if (!preview || !preview.frameUrl) throw new Error("missing preview frameUrl");
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

  function localScaffold(id, name, description) {
    var cleanId = String(id || "my-app").trim().toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "") || "my-app";
    var cleanName = String(name || cleanId).trim() || cleanId;
    var cleanDescription = String(description || "A local-first Terrane app.").trim();
    return {
      id: cleanId,
      name: cleanName,
      files: buildStarterFiles(cleanId, cleanName, cleanDescription)
    };
  }

  function buildStarterFiles(id, name, description) {
    return [
      {
        path: "manifest.json",
        content: JSON.stringify({ id: id, name: name, version: "0.1.0", backend: "main.js", ui: "index.html", resources: [] }, null, 2) + "\n"
      },
      {
        path: "main.js",
        content: [
          "var description = " + JSON.stringify(description || ("A local app named " + name)) + ";",
          "",
          "var actions = {",
          "  hello: {",
          "    summary: \"Return a greeting.\",",
          "    args: [],",
          "    returns: \"a greeting line.\",",
          "    run: function () {",
          "      return \"Hello from " + escapeJsString(name) + "\";",
          "    }",
          "  }",
          "};",
          ""
        ].join("\n")
      },
      {
        path: "index.html",
        content: [
          "<!doctype html>",
          "<html lang=\"en\">",
          "<head>",
          "<meta charset=\"utf-8\">",
          "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
          "<title>" + escapeHtml(name) + "</title>",
          "<link rel=\"stylesheet\" href=\"style.css\">",
          "</head>",
          "<body>",
          "  <main class=\"app\">",
          "    <h1>" + escapeHtml(name) + "</h1>",
          "    <p>" + escapeHtml(description || "A Terrane app.") + "</p>",
          "    <div class=\"actions\">",
          "      <button id=\"hello\" type=\"button\">Run hello</button>",
          "      <output id=\"result\">Ready</output>",
          "    </div>",
          "  </main>",
          "  <script>",
          "    var result = document.getElementById(\"result\");",
          "    function show(value) { result.textContent = value; }",
          "    function runHello() {",
          "      if (!window.terrane || !window.terrane.invoke) { show(\"Preview bridge unavailable\"); return; }",
          "      window.terrane.invoke(\"hello\").then(show).catch(function (error) { show(\"Error: \" + error.message); });",
          "    }",
          "    document.getElementById(\"hello\").addEventListener(\"click\", runHello);",
          "    runHello();",
          "  </script>",
          "</body>",
          "</html>",
          ""
        ].join("\n")
      },
      {
        path: "style.css",
        content: [
          ":root { color-scheme: light dark; }",
          "* { box-sizing: border-box; }",
          "body { margin: 0; font: 14px -apple-system, BlinkMacSystemFont, \"Segoe UI\", sans-serif; background: Canvas; color: CanvasText; }",
          ".app { width: min(680px, calc(100vw - 32px)); margin: 0 auto; padding: 28px 0; }",
          "h1 { margin: 0 0 8px; font-size: 24px; letter-spacing: 0; }",
          "p { margin: 0; color: color-mix(in srgb, CanvasText 65%, transparent); }",
          ".actions { display: flex; align-items: center; gap: 12px; margin-top: 18px; }",
          "button { min-height: 34px; border: 0; border-radius: 8px; padding: 0 12px; background: #0071e3; color: white; font: inherit; font-weight: 700; }",
          "output { color: color-mix(in srgb, CanvasText 70%, transparent); }",
          ""
        ].join("\n")
      }
    ];
  }

  function escapeJsString(input) {
    return String(input || "").replace(/\\/g, "\\\\").replace(/"/g, "\\\"");
  }

  function escapeHtml(input) {
    return String(input || "").replace(/[&<>"']/g, function (ch) {
      return ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[ch];
    });
  }
})();
