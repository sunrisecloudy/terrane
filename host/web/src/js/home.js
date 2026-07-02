(function () {
  var list = document.getElementById("home-app-list");

  fetch("/apps", { cache: "no-store" })
    .then(function (response) {
      if (!response.ok) throw new Error("cannot load apps");
      return response.json();
    })
    .then(function (catalog) {
      renderCatalog(Array.isArray(catalog.apps) ? catalog.apps : []);
    })
    .catch(function () {
      showError("Cannot load apps");
    });

  function renderCatalog(apps) {
    list.replaceChildren();

    apps.forEach(function (app) {
      list.appendChild(appCard(app));
    });

    if (!apps.length) {
      var empty = document.createElement("div");
      empty.className = "app-empty";
      empty.textContent = "No apps installed";
      list.appendChild(empty);
    }
  }

  function appCard(app) {
    var id = app && app.id ? String(app.id) : "";
    var name = app && app.name ? String(app.name) : id || "Unnamed app";
    var root = app && app.has_ui
      ? document.createElement("a")
      : document.createElement("div");
    root.className = "app-card";
    if (app && app.has_ui) {
      root.href = "/apps/" + encodeURIComponent(id) + "/";
    } else {
      root.className += " disabled";
    }

    var label = document.createElement("span");
    label.textContent = name;
    root.appendChild(label);

    var meta = document.createElement("small");
    meta.textContent = app && app.has_ui ? id : id + " - no UI";
    root.appendChild(meta);

    return root;
  }

  function showError(message) {
    list.replaceChildren();
    var error = document.createElement("div");
    error.className = "app-error";
    error.textContent = message;
    list.appendChild(error);
  }
})();
