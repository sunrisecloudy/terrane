// Pixel Paint backend for Terrane.
//
// The UI renders a 64x64 canvas and persists only painted cells. Empty cells are
// transparent in storage and rendered on a white checker/grid surface by the UI.

var kv = ctx.resource.kv;

var SIZE = 64;
var PIXELS_KEY = "pixels";

function readPixels() {
  var raw = kv.get(PIXELS_KEY);
  if (raw == null || raw === "") return {};
  try {
    var parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : {};
  } catch (_e) {
    return {};
  }
}

function writePixels(pixels) {
  kv.set(PIXELS_KEY, JSON.stringify(pixels));
}

function validCoord(n) {
  return Number.isInteger(n) && n >= 0 && n < SIZE;
}

function normalizeColor(color) {
  if (typeof color !== "string") return null;
  var c = color.trim().toLowerCase();
  return /^#[0-9a-f]{6}$/.test(c) ? c : null;
}

function cellKey(x, y) {
  return String(x) + "," + String(y);
}

function applyCell(pixels, x, y, color) {
  if (!validCoord(x) || !validCoord(y)) return false;
  var key = cellKey(x, y);
  if (color == null || color === "") {
    delete pixels[key];
    return true;
  }
  var normalized = normalizeColor(color);
  if (normalized == null) return false;
  pixels[key] = normalized;
  return true;
}

var description = "A 64 by 64 pixel paint canvas with kv-backed persistence.";

var actions = {
  state: {
    summary: "Return the current canvas state as JSON.",
    args: [],
    returns: "JSON with size and sparse pixels map.",
    run: function () {
      return JSON.stringify({ size: SIZE, pixels: readPixels() });
    }
  },

  set: {
    summary: "Set one pixel.",
    args: [
      { name: "x", required: true, summary: "x coordinate, 0-63" },
      { name: "y", required: true, summary: "y coordinate, 0-63" },
      { name: "color", required: true, summary: "hex color like #ff0066, or empty to erase" }
    ],
    returns: "a confirmation line.",
    run: function (args, usage) {
      var x = parseInt(args[0], 10);
      var y = parseInt(args[1], 10);
      var color = args.length >= 3 ? args[2] : "";
      var pixels = readPixels();
      if (!applyCell(pixels, x, y, color)) return usage();
      writePixels(pixels);
      return "set " + x + "," + y;
    }
  },

  bulk: {
    summary: "Apply many pixel changes.",
    args: [{ name: "changes", required: true, summary: "JSON array of {x,y,color}" }],
    returns: "a confirmation with the applied count.",
    run: function (args, usage) {
      var changes;
      try {
        changes = JSON.parse(args[0] || "[]");
      } catch (_e) {
        return usage();
      }
      if (!Array.isArray(changes)) return usage();
      var pixels = readPixels();
      var applied = 0;
      for (var i = 0; i < changes.length; i += 1) {
        var change = changes[i] || {};
        var x = parseInt(change.x, 10);
        var y = parseInt(change.y, 10);
        var color = change.color == null ? "" : String(change.color);
        if (applyCell(pixels, x, y, color)) applied += 1;
      }
      writePixels(pixels);
      return "applied " + applied;
    }
  },

  clear: {
    summary: "Clear the canvas.",
    args: [],
    returns: "a confirmation line.",
    run: function () {
      writePixels({});
      return "cleared";
    }
  }
};
