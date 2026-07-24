// Pixel Paint backend for Terrane.
//
// The UI renders a 64x64 canvas and persists only painted cells. Empty cells are
// transparent in storage and rendered on a white checker/grid surface by the UI.

var kv = ctx.resource.kv;

var SIZE = 64;
var PIXELS_KEY = "pixels";
var DEFAULT_CANVAS = "default";
var CANVAS_SEQ_KEY = "canvas:seq";
var SELECTED_CANVAS_KEY = "canvas:selected";
var CANVAS_PREFIX = "canvas:";

function pixelsKey(id) {
  return id === DEFAULT_CANVAS ? PIXELS_KEY : CANVAS_PREFIX + id + ":pixels";
}

function metaKey(id) {
  return CANVAS_PREFIX + id + ":meta";
}

function readNumber(key) {
  var raw = kv.get(key);
  var n = raw == null ? 0 : parseInt(raw, 10);
  return isNaN(n) || n < 0 ? 0 : n;
}

function canvasIds() {
  var all = kv.all();
  var ids = [DEFAULT_CANVAS];
  for (var key in all) {
    if (!Object.prototype.hasOwnProperty.call(all, key)) continue;
    if (key.indexOf(CANVAS_PREFIX) !== 0 || key.slice(-5) !== ":meta") continue;
    var id = key.slice(CANVAS_PREFIX.length, -5);
    if (id && id !== DEFAULT_CANVAS && ids.indexOf(id) < 0) ids.push(id);
  }
  ids.sort();
  return ids;
}

function selectedCanvas() {
  var id = kv.get(SELECTED_CANVAS_KEY) || DEFAULT_CANVAS;
  return canvasIds().indexOf(id) < 0 ? DEFAULT_CANVAS : id;
}

function canvasTitle(id) {
  var raw = kv.get(metaKey(id));
  if (raw != null) {
    try {
      var meta = JSON.parse(raw);
      if (meta && meta.title) return String(meta.title);
    } catch (_e) {}
  }
  return id === DEFAULT_CANVAS ? "Canvas 1" : "Untitled canvas";
}

function canvases() {
  return canvasIds().map(function (id) {
    return {
      id: id,
      title: canvasTitle(id),
      pixelCount: Object.keys(readPixels(id)).length,
    };
  });
}

function readPixels(id) {
  var raw = kv.get(pixelsKey(id || selectedCanvas()));
  if (raw == null || raw === "") return {};
  try {
    var parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? parsed
      : {};
  } catch (_e) {
    return {};
  }
}

function writePixels(pixels, id) {
  kv.set(pixelsKey(id || selectedCanvas()), JSON.stringify(pixels));
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
      var selected = selectedCanvas();
      return JSON.stringify({
        size: SIZE,
        pixels: readPixels(selected),
        selectedCanvas: selected,
        canvases: canvases(),
      });
    },
  },

  set: {
    summary: "Set one pixel.",
    args: [
      { name: "x", required: true, summary: "x coordinate, 0-63" },
      { name: "y", required: true, summary: "y coordinate, 0-63" },
      {
        name: "color",
        required: true,
        summary: "hex color like #ff0066, or empty to erase",
      },
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
    },
  },

  bulk: {
    summary: "Apply many pixel changes.",
    args: [{
      name: "changes",
      required: true,
      summary: "JSON array of {x,y,color}",
    }],
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
    },
  },

  clear: {
    summary: "Clear the canvas.",
    args: [],
    returns: "a confirmation line.",
    run: function () {
      writePixels({});
      return "cleared";
    },
  },

  "new": {
    summary: "Create and select a new persisted canvas.",
    args: [],
    returns: "the updated canvas state as JSON.",
    run: function () {
      var next = readNumber(CANVAS_SEQ_KEY) + 1;
      var id = "c" + String(next);
      kv.set(CANVAS_SEQ_KEY, String(next));
      kv.set(metaKey(id), JSON.stringify({ title: "Canvas " + String(next + 1) }));
      kv.set(pixelsKey(id), "{}");
      kv.set(SELECTED_CANVAS_KEY, id);
      return actions.state.run();
    },
  },

  select: {
    summary: "Select a persisted canvas by id.",
    args: [{ name: "canvas-id", required: true }],
    returns: "the updated canvas state as JSON.",
    run: function (args, usage) {
      var id = args[0];
      if (!id) return usage();
      if (canvasIds().indexOf(id) < 0) return "unknown canvas: " + id;
      kv.set(SELECTED_CANVAS_KEY, id);
      return actions.state.run();
    },
  },
};
