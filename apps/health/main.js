var kv = ctx.resource.kv;
var blob = ctx.resource.blob;
var model = ctx.resource.model;

var SEQ_KEY = "estimate:seq";
var ENTRY_PREFIX = "estimate:";
var SETTINGS_KEY = "settings:model";
var NAVIGATION_KEY = "navigation:pending";
var MAX_HISTORY = 1000;
var MAX_DISHES = 12;
var MAX_PENDING_NAVIGATIONS = 8;
var DEFAULT_PROVIDER = "opencode";
var DEFAULT_OPENCODE_MODEL = "opencode-go/kimi-k2.6";

var description =
  "Estimate nutrition from a food photo with vision AI, then review and save the result.";

function pad(number) {
  var out = String(number);
  while (out.length < 8) out = "0" + out;
  return out;
}

function nextId() {
  var raw = kv.get(SEQ_KEY);
  var current = raw == null ? 0 : parseInt(raw, 10);
  if (isNaN(current) || current < 0) current = 0;
  var next = current + 1;
  kv.set(SEQ_KEY, String(next));
  return pad(next);
}

function safeText(value, fallback, maxLength) {
  var text = typeof value === "string" ? value.replace(/\s+/g, " ").trim() : "";
  if (!text) text = fallback;
  return text.slice(0, maxLength);
}

function numberInRange(value, min, max, fallback) {
  var numeric = typeof value === "number" ? value : parseFloat(value);
  if (!isFinite(numeric) || numeric < min || numeric > max) return fallback;
  return Math.round(numeric * 10) / 10;
}

function stringList(value, limit) {
  if (!Array.isArray(value)) return [];
  var output = [];
  for (var i = 0; i < value.length && output.length < limit; i++) {
    var item = safeText(value[i], "", 180);
    if (item) output.push(item);
  }
  return output;
}

function normalizeProvider(value) {
  return value === "codex" ? "codex" : DEFAULT_PROVIDER;
}

function normalizeOpenCodeModel(value) {
  var model = safeText(value, DEFAULT_OPENCODE_MODEL, 200);
  if (
    model.indexOf("/") < 1 ||
    !/^[A-Za-z0-9._~:-]+\/[A-Za-z0-9._~:/-]+$/.test(model)
  ) {
    return DEFAULT_OPENCODE_MODEL;
  }
  return model;
}

function normalizeSettings(value) {
  var provider = normalizeProvider(value && value.provider);
  return {
    provider: provider,
    model:
      provider === "opencode"
        ? normalizeOpenCodeModel(value && value.model)
        : ""
  };
}

function readSettings() {
  var raw = kv.get(SETTINGS_KEY);
  if (raw == null) return normalizeSettings(null);
  try {
    return normalizeSettings(JSON.parse(raw));
  } catch (_error) {
    return normalizeSettings(null);
  }
}

function saveSettings(settings) {
  var normalized = normalizeSettings(settings);
  kv.set(SETTINGS_KEY, JSON.stringify(normalized));
  return normalized;
}

function agentSelector(settings) {
  return settings.provider === "codex"
    ? "codex"
    : "opencode:" + settings.model;
}

function normalizeDish(value, index) {
  if (!value || typeof value !== "object") return null;
  var calories = numberInRange(value.calories_kcal, 0, 5000, null);
  if (calories == null) return null;
  return {
    food_name: safeText(value.food_name, "Dish " + (index + 1), 100),
    confidence: numberInRange(value.confidence, 0, 1, 0.35),
    serving_description: safeText(
      value.serving_description,
      "Estimated visible portion",
      140
    ),
    calories_kcal: calories,
    protein_g: numberInRange(value.protein_g, 0, 1000, 0),
    carbs_g: numberInRange(value.carbs_g, 0, 1500, 0),
    fat_g: numberInRange(value.fat_g, 0, 1000, 0),
    fiber_g: numberInRange(value.fiber_g, 0, 500, 0),
    sugar_g: numberInRange(value.sugar_g, 0, 1000, 0),
    sodium_mg: numberInRange(value.sodium_mg, 0, 50000, 0)
  };
}

function roundedSum(dishes, field) {
  var total = 0;
  for (var i = 0; i < dishes.length; i++) total += dishes[i][field] || 0;
  return Math.round(total * 10) / 10;
}

function dishTotals(dishes) {
  return {
    calories_kcal: roundedSum(dishes, "calories_kcal"),
    protein_g: roundedSum(dishes, "protein_g"),
    carbs_g: roundedSum(dishes, "carbs_g"),
    fat_g: roundedSum(dishes, "fat_g"),
    fiber_g: roundedSum(dishes, "fiber_g"),
    sugar_g: roundedSum(dishes, "sugar_g"),
    sodium_mg: roundedSum(dishes, "sodium_mg")
  };
}

function defaultMealName(dishes) {
  if (dishes.length === 1) return dishes[0].food_name;
  if (dishes.length > 1) return "Meal with " + dishes.length + " dishes";
  return "Food photo";
}

function averageDishConfidence(dishes) {
  if (!dishes.length) return 0.35;
  return roundedSum(dishes, "confidence") / dishes.length;
}

function normalizeEstimate(value) {
  if (!value || typeof value !== "object") return null;
  var dishes = [];
  if (Array.isArray(value.dishes)) {
    for (var i = 0; i < value.dishes.length && dishes.length < MAX_DISHES; i++) {
      var dish = normalizeDish(value.dishes[i], dishes.length);
      if (!dish) return null;
      dishes.push(dish);
    }
  }

  var totals;
  if (dishes.length) {
    totals = dishTotals(dishes);
    if (totals.calories_kcal > 10000) return null;
  } else {
    var legacy = normalizeDish(value, 0);
    if (!legacy) return null;
    totals = dishTotals([legacy]);
  }

  return {
    food_name: safeText(value.food_name, defaultMealName(dishes), 100),
    confidence: numberInRange(
      value.confidence,
      0,
      1,
      averageDishConfidence(dishes)
    ),
    serving_description: safeText(
      value.serving_description,
      dishes.length > 1
        ? dishes.length + " separately estimated visible dishes"
        : "Estimated visible serving",
      140
    ),
    calories_kcal: totals.calories_kcal,
    protein_g: totals.protein_g,
    carbs_g: totals.carbs_g,
    fat_g: totals.fat_g,
    fiber_g: totals.fiber_g,
    sugar_g: totals.sugar_g,
    sodium_mg: totals.sodium_mg,
    dishes: dishes,
    assumptions: stringList(value.assumptions, 6),
    warnings: stringList(value.warnings, 6)
  };
}

function parseModelJson(raw) {
  var text = String(raw == null ? "" : raw).trim();
  if (text.indexOf("```") === 0) {
    text = text.replace(/^```(?:json)?\s*/i, "").replace(/\s*```$/, "");
  }
  try {
    return JSON.parse(text);
  } catch (_first) {
    var start = text.indexOf("{");
    var end = text.lastIndexOf("}");
    if (start >= 0 && end > start) {
      try {
        return JSON.parse(text.slice(start, end + 1));
      } catch (_second) {}
    }
  }
  return null;
}

function promptFor(note) {
  var context = safeText(note, "No extra meal details were provided.", 500);
  return [
    "Analyze the attached food photo only. Identify every distinct visible dish or meaningful",
    "food component and estimate each one separately. Do not merge separate dishes into one item.",
    "Use visual portion cues and state important uncertainty.",
    "User note: " + context,
    "Return exactly one JSON object and no markdown or commentary.",
    "Schema:",
    '{"food_name":"short overall meal name","confidence":0.0,',
    '"serving_description":"overall visible serving","dishes":[',
    '{"food_name":"distinct dish name","confidence":0.0,"serving_description":"visible portion",',
    '"calories_kcal":0,"protein_g":0,"carbs_g":0,"fat_g":0,"fiber_g":0,',
    '"sugar_g":0,"sodium_mg":0}],"assumptions":["..."],"warnings":["..."]}',
    "Return one dishes entry for each distinct visible dish, up to 12. A single-dish photo must",
    "still return one dishes entry. Confidence must be from 0 to 1. Every nutrient must be a",
    "non-negative number. Do not return combined nutrient totals; Terrane calculates the meal",
    "total from the separate dishes.",
    "Do not diagnose, prescribe, or claim laboratory accuracy. If the image is not food,",
    'return one dish with calories_kcal 0, confidence 0, and include "No food clearly visible"',
    "in warnings."
  ].join(" ");
}

function historyEntries() {
  var all = kv.all();
  var keys = [];
  for (var key in all) {
    if (
      Object.prototype.hasOwnProperty.call(all, key) &&
      key.indexOf(ENTRY_PREFIX) === 0 &&
      key !== SEQ_KEY
    ) {
      keys.push(key);
    }
  }
  keys.sort();
  keys.reverse();
  var entries = [];
  for (var i = 0; i < keys.length && entries.length < MAX_HISTORY; i++) {
    try {
      entries.push(JSON.parse(all[keys[i]]));
    } catch (_error) {}
  }
  return entries;
}

function stateJson() {
  return JSON.stringify({
    ok: true,
    settings: readSettings(),
    history: historyEntries()
  });
}

function saveEntry(entry) {
  kv.set(ENTRY_PREFIX + entry.id, JSON.stringify(entry));
}

function normalizeEatenAt(value, fallback) {
  var candidate = typeof value === "string" ? value : "";
  var date = new Date(candidate);
  if (candidate && isFinite(date.getTime())) return date.toISOString();
  var fallbackDate = new Date(fallback || "");
  return isFinite(fallbackDate.getTime())
    ? fallbackDate.toISOString()
    : new Date().toISOString();
}

function isPickerImport(name) {
  return /^imports\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.jpg$/.test(
    String(name || "")
  );
}

function blobIsReferenced(name) {
  var all = kv.all();
  for (var key in all) {
    if (
      Object.prototype.hasOwnProperty.call(all, key) &&
      key.indexOf(ENTRY_PREFIX) === 0 &&
      key !== SEQ_KEY
    ) {
      try {
        if (JSON.parse(all[key]).blob_name === name) return true;
      } catch (_error) {}
    }
  }
  return false;
}

function removeOwnedUnreferencedImport(name) {
  if (!isPickerImport(name) || blobIsReferenced(name)) return false;
  try {
    blob.rm(name);
    return true;
  } catch (_error) {
    return false;
  }
}

function analyzeStoredBlob(blobName, note, settings, id) {
  var request = JSON.stringify({
    parts: [{ text: promptFor(note) }, { blob: blobName }]
  });
  var raw = model.ask(agentSelector(settings), request);
  var normalized = normalizeEstimate(parseModelJson(raw));
  if (!normalized) {
    return null;
  }
  var entry = normalized;
  entry.id = id;
  entry.blob_name = blobName;
  entry.note = safeText(note, "", 500);
  entry.provider = settings.provider;
  entry.model = settings.model;
  entry.eaten_at = new Date().toISOString();
  entry.reviewed = false;
  saveEntry(entry);
  return entry;
}

function estimate(args, usage) {
  if (args.length < 2) return usage();
  var base64 = String(args[0] || "");
  var mime = String(args[1] || "").toLowerCase();
  var note = args.length > 2 ? args[2] : "";
  var settings = saveSettings({
    provider: args.length > 3 ? args[3] : readSettings().provider,
    model: args.length > 4 ? args[4] : readSettings().model
  });
  if (!base64 || base64.length > 12 * 1024 * 1024) {
    return JSON.stringify({ ok: false, error: "The prepared image is missing or too large." });
  }
  if (mime !== "image/jpeg" && mime !== "image/png" && mime !== "image/webp") {
    return JSON.stringify({ ok: false, error: "Use a JPEG, PNG, or WebP food photo." });
  }

  var id = nextId();
  var extension = mime === "image/png" ? "png" : mime === "image/webp" ? "webp" : "jpg";
  var blobName = "meal/" + id + "." + extension;
  try {
    blob.put(blobName, base64, mime);
    var entry = analyzeStoredBlob(blobName, note, settings, id);
    if (!entry) {
      blob.rm(blobName);
      return JSON.stringify({
        ok: false,
        error: "Vision analysis returned an unreadable result. Please try another photo."
      });
    }
    return JSON.stringify({ ok: true, estimate: entry, history: historyEntries() });
  } catch (error) {
    try {
      blob.rm(blobName);
    } catch (_cleanupError) {}
    return JSON.stringify({
      ok: false,
      error:
        "Vision analysis is unavailable. Check that the selected provider is signed in, then try again. " +
        safeText(error && error.message, "", 180)
    });
  }
}

function estimateBlob(args, usage) {
  if (args.length < 1) return usage();
  var blobName = String(args[0] || "");
  var note = args.length > 1 ? args[1] : "";
  var settings = saveSettings({
    provider: args.length > 2 ? args[2] : readSettings().provider,
    model: args.length > 3 ? args[3] : readSettings().model
  });
  if (!isPickerImport(blobName)) {
    return JSON.stringify({ ok: false, error: "The selected Photos import is invalid." });
  }
  try {
    // Stat through the app resource boundary before handing the canonical blob
    // reference to the model multipart path.
    var metadata = JSON.parse(blob.stat(blobName));
    if (
      !metadata ||
      metadata.name !== blobName ||
      metadata.mime !== "image/jpeg" ||
      metadata.size <= 0 ||
      metadata.size > 10 * 1024 * 1024
    ) {
      throw new Error("invalid imported image metadata");
    }
    var id = nextId();
    var entry = analyzeStoredBlob(blobName, note, settings, id);
    if (!entry) {
      removeOwnedUnreferencedImport(blobName);
      return JSON.stringify({
        ok: false,
        error: "Vision analysis returned an unreadable result. Please choose the photo again."
      });
    }
    return JSON.stringify({ ok: true, estimate: entry, history: historyEntries() });
  } catch (error) {
    removeOwnedUnreferencedImport(blobName);
    return JSON.stringify({
      ok: false,
      error:
        "Vision analysis is unavailable. Check that the selected provider is signed in, then try again. " +
        safeText(error && error.message, "", 180)
    });
  }
}

function discardImport(args, usage) {
  if (args.length < 1) return usage();
  return JSON.stringify({
    ok: true,
    removed: removeOwnedUnreferencedImport(String(args[0] || ""))
  });
}

function update(args, usage) {
  if (args.length < 2) return usage();
  var id = String(args[0] || "");
  var raw = kv.get(ENTRY_PREFIX + id);
  if (raw == null) return JSON.stringify({ ok: false, error: "Estimate not found." });
  var existing;
  var changes;
  try {
    existing = JSON.parse(raw);
    changes = JSON.parse(args[1]);
  } catch (_error) {
    return JSON.stringify({ ok: false, error: "Invalid nutrition update." });
  }
  var normalized = normalizeEstimate(changes);
  if (!normalized) {
    return JSON.stringify({ ok: false, error: "Calories and nutrients must be valid numbers." });
  }
  normalized.id = existing.id;
  normalized.blob_name = existing.blob_name;
  normalized.note = existing.note || "";
  normalized.provider = existing.provider || DEFAULT_PROVIDER;
  normalized.model = existing.model || DEFAULT_OPENCODE_MODEL;
  normalized.eaten_at = normalizeEatenAt(changes.eaten_at, existing.eaten_at);
  normalized.reviewed = true;
  saveEntry(normalized);
  return JSON.stringify({ ok: true, estimate: normalized, history: historyEntries() });
}

function remove(args, usage) {
  if (args.length < 1) return usage();
  var id = String(args[0] || "");
  var raw = kv.get(ENTRY_PREFIX + id);
  if (raw == null) return JSON.stringify({ ok: false, error: "Estimate not found." });
  try {
    var entry = JSON.parse(raw);
    if (entry.blob_name) blob.rm(entry.blob_name);
  } catch (_error) {}
  kv.rm(ENTRY_PREFIX + id);
  return stateJson();
}

function configure(args) {
  var settings = saveSettings({
    provider: args.length > 0 ? args[0] : DEFAULT_PROVIDER,
    model: args.length > 1 ? args[1] : DEFAULT_OPENCODE_MODEL
  });
  return JSON.stringify({ ok: true, settings: settings });
}

function routeParams(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  var output = {};
  var count = 0;
  for (var key in value) {
    if (!Object.prototype.hasOwnProperty.call(value, key) || count >= 12) continue;
    if (!/^[a-z][a-z0-9_-]{0,31}$/.test(key)) continue;
    var item = value[key];
    if (typeof item !== "string" || item.length > 128) continue;
    output[key] = item;
    count += 1;
  }
  return output;
}

function validateNavigation(value) {
  if (!value || typeof value !== "object") return null;
  if (typeof value.item === "string") {
    var legacyId = safeText(value.item, "", 128);
    return /^[A-Za-z0-9._-]+$/.test(legacyId)
      ? { route: "meal", segments: [legacyId], params: {} }
      : null;
  }
  var allowed = {
    add: true,
    calendar: true,
    history: true,
    insights: true,
    settings: true,
    meal: true
  };
  var route = typeof value.route === "string" ? value.route : "";
  if (!allowed[route]) return null;
  var inputSegments = Array.isArray(value.segments) ? value.segments : [];
  var segments = [];
  for (var i = 0; i < inputSegments.length && i < 4; i++) {
    var segment = safeText(inputSegments[i], "", 128);
    if (!/^[A-Za-z0-9._-]+$/.test(segment)) return null;
    segments.push(segment);
  }
  if (route === "meal" ? segments.length !== 1 : segments.length !== 0) return null;
  var params = routeParams(value.params);
  if (params.date && !/^\d{4}-\d{2}-\d{2}$/.test(params.date)) return null;
  if (
    params.period &&
    params.period !== "day" &&
    params.period !== "week" &&
    params.period !== "month"
  ) {
    return null;
  }
  return { route: route, segments: segments, params: params };
}

function pendingNavigations() {
  var raw = kv.get(NAVIGATION_KEY);
  if (raw == null) return [];
  try {
    var value = JSON.parse(raw);
    return Array.isArray(value) ? value.slice(0, MAX_PENDING_NAVIGATIONS) : [];
  } catch (_error) {
    return [];
  }
}

function savePendingNavigations(items) {
  if (!items.length) {
    kv.rm(NAVIGATION_KEY);
    return;
  }
  kv.set(NAVIGATION_KEY, JSON.stringify(items.slice(-MAX_PENDING_NAVIGATIONS)));
}

function receiveLink(args, usage) {
  if (args.length < 2) return usage();
  if (args[0] !== "link") {
    return JSON.stringify({ ok: false, error: "Health only accepts link messages." });
  }
  var value;
  try {
    value = JSON.parse(args[1]);
  } catch (_error) {
    return JSON.stringify({ ok: false, error: "Health link payload is not valid JSON." });
  }
  var navigation = validateNavigation(value);
  if (!navigation) {
    return JSON.stringify({ ok: false, error: "Health link route is unsupported or malformed." });
  }
  var pending = pendingNavigations();
  pending.push(navigation);
  savePendingNavigations(pending);
  return JSON.stringify({ ok: true });
}

function consumeNavigation() {
  var pending = pendingNavigations();
  var navigation = pending.shift() || null;
  savePendingNavigations(pending);
  return JSON.stringify({ ok: true, navigation: navigation });
}

function commonList() {
  var entries = historyEntries();
  var items = [];
  for (var i = 0; i < entries.length; i++) {
    items.push({
      id: entries[i].id,
      title: entries[i].food_name,
      kind: "meal"
    });
  }
  return JSON.stringify(items);
}

function commonGet(args, usage) {
  if (args.length < 1) return usage();
  var id = String(args[0] || "");
  var raw = kv.get(ENTRY_PREFIX + id);
  if (raw == null) {
    return JSON.stringify({ ok: false, error: { code: "NotFound", id: id } });
  }
  return raw;
}

var actions = {
  state: {
    summary: "Return saved nutrition estimate history.",
    args: [],
    returns: "JSON state.",
    run: function () {
      return stateJson();
    }
  },
  estimate: {
    summary: "Store a food image and estimate nutrition with the selected vision provider.",
    args: [
      { name: "image_base64", required: true },
      { name: "mime", required: true },
      { name: "meal_note", required: false },
      { name: "provider", required: false },
      { name: "model", required: false }
    ],
    returns: "JSON estimate and history.",
    run: estimate
  },
  estimate_blob: {
    summary: "Estimate nutrition from an already-stored Photos picker blob.",
    args: [
      { name: "blob_name", required: true },
      { name: "meal_note", required: false },
      { name: "provider", required: false },
      { name: "model", required: false }
    ],
    returns: "JSON estimate and history.",
    run: estimateBlob
  },
  discard_import: {
    summary: "Remove an unreferenced Health-owned picker import.",
    args: [{ name: "blob_name", required: true }],
    returns: "JSON cleanup result.",
    run: discardImport
  },
  configure: {
    summary: "Save the default vision provider and model for Health.",
    args: [
      { name: "provider", required: true },
      { name: "model", required: false }
    ],
    returns: "JSON settings.",
    run: configure
  },
  update: {
    summary: "Save a user-reviewed nutrition estimate.",
    args: [
      { name: "id", required: true },
      { name: "nutrition_json", required: true }
    ],
    returns: "JSON estimate and history.",
    run: update
  },
  remove: {
    summary: "Delete one saved estimate and its photo.",
    args: [{ name: "id", required: true }],
    returns: "JSON state.",
    run: remove
  },
  "navigation.consume": {
    summary: "Consume one validated pending app route.",
    args: [],
    returns: "JSON navigation result.",
    run: consumeNavigation
  },
  "common.receive": {
    summary: "Receive a validated Terrane link message.",
    args: [
      { name: "kind", required: true },
      { name: "payload", required: true }
    ],
    returns: "JSON receipt.",
    run: receiveLink
  },
  "common.list": {
    summary: "List saved meals for Terrane item integrations.",
    args: [],
    returns: "JSON item list.",
    run: commonList
  },
  "common.get": {
    summary: "Get one saved meal by id.",
    args: [{ name: "id", required: true }],
    returns: "JSON item.",
    run: commonGet
  }
};
