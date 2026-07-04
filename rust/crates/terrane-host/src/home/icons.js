(function () {
  var DEFAULT_GLYPH =
    '<rect x="3.5" y="3.5" width="17" height="17" rx="4.5" stroke-dasharray="3.5 3.5"/>';

  function iconSource(app) {
    if (!app || typeof app !== "object") return "";
    var value = typeof app.icon === "string" ? app.icon.trim() : "";
    if (!value) return "";
    if (/^data:image\//i.test(value) || /^https?:\/\//i.test(value) || value[0] === "/") {
      return value;
    }
    if (!app.id || value.indexOf("..") !== -1 || value.indexOf("\\") !== -1 || value.indexOf("://") !== -1) {
      return "";
    }
    if (/[\u0000-\u001f\u007f]/.test(value)) return "";
    return "/apps/" + encodeURIComponent(String(app.id)) + "/" + value.replace(/^\/+/, "");
  }

  function fallbackSvg() {
    var svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("stroke-width", "1.8");
    svg.setAttribute("stroke-linecap", "round");
    svg.setAttribute("stroke-linejoin", "round");
    svg.innerHTML = DEFAULT_GLYPH;
    return svg;
  }

  window.terraneAppIcon = function (app) {
    var icon = document.createElement("span");
    icon.className = "app-icon";
    icon.setAttribute("aria-hidden", "true");
    var src = iconSource(app);
    if (src) {
      var glyph = document.createElement("span");
      glyph.className = "app-icon-glyph";
      glyph.style.maskImage = "url(\"" + src.replace(/"/g, "%22") + "\")";
      glyph.style.webkitMaskImage = glyph.style.maskImage;
      icon.appendChild(glyph);
    } else {
      icon.appendChild(fallbackSvg());
    }
    return icon;
  };
})();
