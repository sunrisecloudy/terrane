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

function buildFiles(id, name, description) {
  var manifest = {
    id: id,
    name: name,
    version: "0.1.0",
    backend: "main.js",
    ui: "index.html",
    resources: [],
  };
  return [
    {
      path: "manifest.json",
      content: JSON.stringify(manifest, null, 2) + "\n",
    },
    {
      path: "main.js",
      content: [
        "var description = " +
        JSON.stringify(description || ("A local app named " + name)) + ";",
        "",
        "var actions = {",
        "  hello: {",
        '    summary: "Return a greeting.",',
        "    args: [],",
        '    returns: "a greeting line.",',
        "    run: function () {",
        '      return "Hello from ' +
        name.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '";',
        "    }",
        "  }",
        "};",
        "",
      ].join("\n"),
    },
    {
      path: "index.html",
      content: [
        "<!doctype html>",
        '<html lang="en">',
        "<head>",
        '<meta charset="utf-8">',
        '<meta name="viewport" content="width=device-width, initial-scale=1">',
        "<title>" + escapeHtml(name) + "</title>",
        '<link rel="stylesheet" href="style.css">',
        "</head>",
        "<body>",
        '  <main class="app">',
        "    <h1>" + escapeHtml(name) + "</h1>",
        "    <p>" + escapeHtml(description || "A Terrane app.") + "</p>",
        '    <div class="actions">',
        '      <button id="hello" type="button">Run hello</button>',
        '      <output id="result">Ready</output>',
        "    </div>",
        "  </main>",
        "  <script>",
        '    var result = document.getElementById("result");',
        "    function show(value) { result.textContent = value; }",
        "    function runHello() {",
        '      if (!window.terrane || !window.terrane.invoke) { show("Preview bridge unavailable"); return; }',
        '      window.terrane.invoke("hello").then(show).catch(function (error) { show("Error: " + error.message); });',
        "    }",
        '    document.getElementById("hello").addEventListener("click", runHello);',
        "    runHello();",
        "  </script>",
        "</body>",
        "</html>",
        "",
      ].join("\n"),
    },
    {
      path: "style.css",
      content: [
        ":root { color-scheme: light dark; }",
        "* { box-sizing: border-box; }",
        'body { margin: 0; font: 14px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: Canvas; color: CanvasText; }',
        ".app { width: min(680px, calc(100vw - 32px)); margin: 0 auto; padding: 28px 0; }",
        "h1 { margin: 0 0 8px; font-size: 24px; letter-spacing: 0; }",
        "p { margin: 0; color: color-mix(in srgb, CanvasText 65%, transparent); }",
        ".actions { display: flex; align-items: center; gap: 12px; margin-top: 18px; }",
        "button { min-height: 34px; border: 0; border-radius: 8px; padding: 0 12px; background: #0071e3; color: white; font: inherit; font-weight: 700; }",
        "output { color: color-mix(in srgb, CanvasText 70%, transparent); }",
        "",
      ].join("\n"),
    },
  ];
}

function escapeHtml(input) {
  return String(input || "").replace(/[&<>"']/g, function (ch) {
    return ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    })[ch];
  });
}

var description =
  "Generate a starter Terrane app bundle from a name and description.";

var actions = {
  scaffold: {
    summary: "Generate starter app files.",
    args: [
      { name: "id", required: true, summary: "app id, e.g. grocery-list" },
      { name: "name", required: true, summary: "display name" },
      {
        name: "description",
        required: false,
        summary: "short app description",
      },
    ],
    returns: "JSON with generated files.",
    run: function (args, usage) {
      var id = slug(args[0]);
      if (!id) return usage();
      var name = title(args[1], id);
      var desc = title(args[2], "");
      return JSON.stringify({
        id: id,
        name: name,
        files: buildFiles(id, name, desc),
      });
    },
  },
};
