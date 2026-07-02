(function () {
  // App glyphs, mirroring the macOS host's SF Symbol mapping
  // (AppSidebarView.iconName(for:)): hammer, gauge, palette, checklist,
  // dashed app square as the default. Constant markup only — app ids are
  // used as lookup keys, never interpolated.
  var CHECKLIST =
    '<path d="m3 17 2 2 4-4"/><path d="m3 7 2 2 4-4"/>' +
    '<path d="M13 6h8"/><path d="M13 12h8"/><path d="M13 18h8"/>';
  var GLYPHS = {
    "app-builder":
      '<path d="m15 12-8.4 8.4a2.1 2.1 0 1 1-3-3L12 9"/>' +
      '<path d="m18 15 4-4"/>' +
      '<path d="m21.5 11.5-1.9-1.9A2 2 0 0 1 19 8.2V7l-2.3-2.3a6 6 0 0 0-4.2-1.7L9 3l.9.8A6.2 6.2 0 0 1 12 8.4V10l2 2h1.2a2 2 0 0 1 1.4.6l1.9 1.9"/>',
    "bmi-calculator":
      '<path d="m12 14 4-4"/><path d="M3.3 19a10 10 0 1 1 17.4 0"/>',
    "pixel-paint":
      '<circle cx="13.5" cy="6.5" r=".5"/><circle cx="17.5" cy="10.5" r=".5"/>' +
      '<circle cx="8.5" cy="7.5" r=".5"/><circle cx="6.5" cy="12.5" r=".5"/>' +
      '<path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.9 0 1.6-.7 1.6-1.7 0-.4-.2-.8-.4-1.1-.3-.3-.4-.7-.4-1.1a1.6 1.6 0 0 1 1.7-1.7h2c3 0 5.5-2.5 5.5-5.6C22 6 17.5 2 12 2z"/>',
    todo: CHECKLIST,
    "todo-cli": CHECKLIST,
    "todo-cli-collaborate": CHECKLIST,
  };
  var DEFAULT_GLYPH =
    '<rect x="3.5" y="3.5" width="17" height="17" rx="4.5" stroke-dasharray="3.5 3.5"/>';

  window.terraneAppIcon = function (id) {
    var icon = document.createElement("span");
    icon.className = "app-icon";
    icon.setAttribute("aria-hidden", "true");
    icon.innerHTML =
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor"' +
      ' stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">' +
      (GLYPHS[String(id)] || DEFAULT_GLYPH) +
      "</svg>";
    return icon;
  };
})();
