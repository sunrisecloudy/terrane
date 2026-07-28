var kv = ctx.resource.kv;
var blob = ctx.resource.blob;
var model = ctx.resource.model;

var EXPENSE_SEQ_KEY = "expense:seq";
var EXPENSE_PREFIX = "expense:";
var POCKET_SEQ_KEY = "pocket:seq";
var POCKET_PREFIX = "pocket:";
var SETTINGS_KEY = "settings:model";
var MAX_EXPENSES = 250;
var DEFAULT_PROVIDER = "opencode";
var DEFAULT_OPENCODE_MODEL = "opencode-go/kimi-k2.6";
var CATEGORIES = [
  "Office supplies",
  "Software",
  "Travel",
  "Meals",
  "Utilities",
  "Marketing",
  "Professional services",
  "Equipment",
  "Rent",
  "Other"
];
var EXPENSE_TYPES = ["Operating", "Capital", "Reimbursable", "Tax", "Other"];
var PAYMENT_STATUSES = ["Paid", "Pending", "Reimbursable"];

var description =
  "Analyze office invoices with vision AI, review structured expenses, and plan goal-based budget pockets.";

function kvGet(key) {
  try {
    return kv.get(key);
  } catch (error) {
    if (String(error).indexOf("not found") !== -1) return null;
    throw error;
  }
}

function pad(number) {
  var output = String(number);
  while (output.length < 8) output = "0" + output;
  return output;
}

function nextId(key) {
  var raw = kvGet(key);
  var current = raw == null ? 0 : parseInt(raw, 10);
  if (!isFinite(current) || current < 0) current = 0;
  var next = current + 1;
  kv.set(key, String(next));
  return pad(next);
}

function safeText(value, fallback, maxLength) {
  var text = typeof value === "string" ? value.replace(/\s+/g, " ").trim() : "";
  return (text || fallback).slice(0, maxLength);
}

function money(value, fallback) {
  var number = typeof value === "number" ? value : parseFloat(value);
  if (!isFinite(number) || number < 0 || number > 1000000000) return fallback;
  return Math.round(number * 100) / 100;
}

function confidence(value) {
  var number = typeof value === "number" ? value : parseFloat(value);
  if (!isFinite(number) || number < 0 || number > 1) return 0.35;
  return Math.round(number * 100) / 100;
}

function oneOf(value, allowed, fallback) {
  var text = safeText(value, "", 80);
  return allowed.indexOf(text) >= 0 ? text : fallback;
}

function isoDate(value) {
  var text = safeText(value, "", 10);
  return /^\d{4}-\d{2}-\d{2}$/.test(text) ? text : "";
}

function currency(value) {
  var text = safeText(value, "USD", 3).toUpperCase();
  return /^[A-Z]{3}$/.test(text) ? text : "USD";
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

function normalizeLineItems(value) {
  if (!Array.isArray(value)) return [];
  var output = [];
  for (var i = 0; i < value.length && output.length < 40; i++) {
    var item = value[i];
    if (!item || typeof item !== "object") continue;
    output.push({
      description: safeText(item.description, "Invoice item", 160),
      quantity: money(item.quantity, 1),
      unit_price: money(item.unit_price, 0),
      amount: money(item.amount, 0)
    });
  }
  return output;
}

function normalizeExpense(value) {
  if (!value || typeof value !== "object") return null;
  var total = money(value.total, null);
  if (total == null) return null;
  return {
    vendor: safeText(value.vendor, "Unknown vendor", 120),
    invoice_number: safeText(value.invoice_number, "", 80),
    invoice_date: isoDate(value.invoice_date),
    due_date: isoDate(value.due_date),
    currency: currency(value.currency),
    subtotal: money(value.subtotal, total),
    tax: money(value.tax, 0),
    total: total,
    category: oneOf(value.category, CATEGORIES, "Other"),
    expense_type: oneOf(value.expense_type, EXPENSE_TYPES, "Operating"),
    payment_status: oneOf(value.payment_status, PAYMENT_STATUSES, "Pending"),
    description: safeText(value.description, "Office expense", 240),
    line_items: normalizeLineItems(value.line_items),
    confidence: confidence(value.confidence),
    warnings: stringList(value.warnings, 8)
  };
}

function normalizeProvider(value) {
  return value === "codex" ? "codex" : DEFAULT_PROVIDER;
}

function normalizeOpenCodeModel(value) {
  var selected = safeText(value, DEFAULT_OPENCODE_MODEL, 200);
  if (
    selected.indexOf("/") < 1 ||
    !/^[A-Za-z0-9._~:-]+\/[A-Za-z0-9._~:/-]+$/.test(selected)
  ) {
    return DEFAULT_OPENCODE_MODEL;
  }
  return selected;
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
  var raw = kvGet(SETTINGS_KEY);
  if (raw == null) return normalizeSettings(null);
  try {
    return normalizeSettings(JSON.parse(raw));
  } catch (_error) {
    return normalizeSettings(null);
  }
}

function saveSettings(value) {
  var settings = normalizeSettings(value);
  kv.set(SETTINGS_KEY, JSON.stringify(settings));
  return settings;
}

function agentSelector(settings) {
  return settings.provider === "codex"
    ? "codex"
    : "opencode:" + settings.model;
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
  var context = safeText(note, "No extra context was provided.", 500);
  return [
    "Analyze only the attached office receipt or invoice image.",
    "Transcribe the accounting fields and visible line items. Do not invent unreadable values.",
    "Use ISO dates (YYYY-MM-DD) and an ISO 4217 currency code.",
    "User context: " + context,
    "Return exactly one JSON object with no markdown.",
    "Schema:",
    '{"vendor":"merchant or supplier","invoice_number":"","invoice_date":"YYYY-MM-DD",',
    '"due_date":"YYYY-MM-DD","currency":"USD","subtotal":0,"tax":0,"total":0,',
    '"category":"Office supplies|Software|Travel|Meals|Utilities|Marketing|Professional services|Equipment|Rent|Other",',
    '"expense_type":"Operating|Capital|Reimbursable|Tax|Other",',
    '"payment_status":"Paid|Pending|Reimbursable","description":"short purpose",',
    '"line_items":[{"description":"","quantity":1,"unit_price":0,"amount":0}],',
    '"confidence":0.0,"warnings":["uncertainty or validation issue"]}',
    "Every amount must be non-negative and confidence must be from 0 to 1.",
    "If this is not an invoice or receipt, return total 0, confidence 0, category Other,",
    'and include "No invoice or receipt clearly visible" in warnings.'
  ].join(" ");
}

function collect(prefix, limit) {
  var all = kv.all();
  var keys = [];
  for (var key in all) {
    if (
      Object.prototype.hasOwnProperty.call(all, key) &&
      key.indexOf(prefix) === 0 &&
      key !== EXPENSE_SEQ_KEY &&
      key !== POCKET_SEQ_KEY
    ) {
      keys.push(key);
    }
  }
  keys.sort();
  keys.reverse();
  var output = [];
  for (var i = 0; i < keys.length && output.length < limit; i++) {
    try {
      output.push(JSON.parse(all[keys[i]]));
    } catch (_error) {}
  }
  return output;
}

function expenses() {
  return collect(EXPENSE_PREFIX, MAX_EXPENSES);
}

function pockets() {
  return collect(POCKET_PREFIX, 50);
}

function summary(items) {
  var total = 0;
  var pending = 0;
  var reimbursable = 0;
  var byCategory = {};
  for (var i = 0; i < items.length; i++) {
    var amount = money(items[i].total, 0);
    total += amount;
    if (items[i].payment_status === "Pending") pending += amount;
    if (items[i].payment_status === "Reimbursable") reimbursable += amount;
    byCategory[items[i].category || "Other"] =
      (byCategory[items[i].category || "Other"] || 0) + amount;
  }
  for (var category in byCategory) {
    if (Object.prototype.hasOwnProperty.call(byCategory, category)) {
      byCategory[category] = Math.round(byCategory[category] * 100) / 100;
    }
  }
  return {
    total: Math.round(total * 100) / 100,
    pending: Math.round(pending * 100) / 100,
    reimbursable: Math.round(reimbursable * 100) / 100,
    count: items.length,
    by_category: byCategory
  };
}

function stateJson() {
  var items = expenses();
  return JSON.stringify({
    ok: true,
    settings: readSettings(),
    expenses: items,
    pockets: pockets(),
    summary: summary(items),
    categories: CATEGORIES,
    expense_types: EXPENSE_TYPES,
    payment_statuses: PAYMENT_STATUSES
  });
}

function saveExpense(entry) {
  kv.set(EXPENSE_PREFIX + entry.id, JSON.stringify(entry));
}

function analyzeInvoice(args, usage) {
  if (args.length < 2) return usage();
  var base64 = String(args[0] || "");
  var mime = String(args[1] || "").toLowerCase();
  var note = args.length > 2 ? args[2] : "";
  var settings = saveSettings({
    provider: args.length > 3 ? args[3] : readSettings().provider,
    model: args.length > 4 ? args[4] : readSettings().model
  });
  var previewOnly = String(args.length > 5 ? args[5] : "").toLowerCase() === "preview";
  if (!base64 || base64.length > 12 * 1024 * 1024) {
    return JSON.stringify({ ok: false, error: "The prepared invoice image is missing or too large." });
  }
  if (["image/jpeg", "image/png", "image/webp"].indexOf(mime) < 0) {
    return JSON.stringify({ ok: false, error: "Use a JPEG, PNG, or WebP invoice image." });
  }

  var id = nextId(EXPENSE_SEQ_KEY);
  var extension = mime === "image/png" ? "png" : mime === "image/webp" ? "webp" : "jpg";
  var blobName = "invoice/" + id + "." + extension;
  try {
    blob.put(blobName, base64, mime);
    var request = JSON.stringify({
      parts: [{ text: promptFor(note) }, { blob: blobName }]
    });
    var raw = model.ask(agentSelector(settings), request);
    var entry = normalizeExpense(parseModelJson(raw));
    if (!entry) {
      blob.rm(blobName);
      return JSON.stringify({
        ok: false,
        error: "Invoice analysis returned an unreadable result. Please try another image."
      });
    }
    entry.id = id;
    entry.blob_name = blobName;
    entry.source = "invoice";
    entry.note = safeText(note, "", 500);
    entry.provider = settings.provider;
    entry.model = settings.model;
    entry.reviewed = false;
    entry.created_at = new Date().toISOString();
    if (!previewOnly) saveExpense(entry);
    return stateJsonWith(entry);
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

function analyzeBlob(args, usage) {
  if (args.length < 1) return usage();
  var blobName = safeText(args[0], "", 240);
  var note = args.length > 1 ? args[1] : "";
  var settings = saveSettings({
    provider: args.length > 2 ? args[2] : readSettings().provider,
    model: args.length > 3 ? args[3] : readSettings().model
  });
  if (!blobName) {
    return JSON.stringify({ ok: false, error: "Pass an app-local invoice blob name." });
  }
  try {
    var metadata = JSON.parse(blob.stat(blobName));
    var mime = String(metadata.mime || "").toLowerCase();
    if (["image/jpeg", "image/png", "image/webp"].indexOf(mime) < 0) {
      return JSON.stringify({
        ok: false,
        error: "The imported blob must be a JPEG, PNG, or WebP invoice image."
      });
    }
    var request = JSON.stringify({
      parts: [{ text: promptFor(note) }, { blob: blobName }]
    });
    var raw = model.ask(agentSelector(settings), request);
    var entry = normalizeExpense(parseModelJson(raw));
    if (!entry) {
      return JSON.stringify({
        ok: false,
        error: "Invoice analysis returned an unreadable result. Please try another image."
      });
    }
    entry.id = nextId(EXPENSE_SEQ_KEY);
    entry.blob_name = blobName;
    entry.source = "invoice";
    entry.note = safeText(note, "", 500);
    entry.provider = settings.provider;
    entry.model = settings.model;
    entry.reviewed = false;
    entry.created_at = new Date().toISOString();
    saveExpense(entry);
    return stateJsonWith(entry);
  } catch (error) {
    return JSON.stringify({
      ok: false,
      error:
        "Could not analyze the imported invoice blob. " +
        safeText(error && error.message, "", 180)
    });
  }
}

function stateJsonWith(expense) {
  var state = JSON.parse(stateJson());
  state.expense = expense;
  return JSON.stringify(state);
}

function createManual(args, usage) {
  if (args.length < 1) return usage();
  var parsed;
  try {
    parsed = JSON.parse(args[0]);
  } catch (_error) {
    return JSON.stringify({ ok: false, error: "Invalid expense details." });
  }
  var entry = normalizeExpense(parsed);
  if (!entry) return JSON.stringify({ ok: false, error: "Enter a valid total." });
  entry.id = nextId(EXPENSE_SEQ_KEY);
  entry.blob_name = "";
  entry.source = "manual";
  entry.note = "";
  entry.provider = "";
  entry.model = "";
  entry.reviewed = true;
  entry.created_at = new Date().toISOString();
  saveExpense(entry);
  return stateJsonWith(entry);
}

function updateExpense(args, usage) {
  if (args.length < 2) return usage();
  var id = String(args[0] || "");
  var raw = kvGet(EXPENSE_PREFIX + id);
  var existing;
  var parsed;
  try {
    parsed = JSON.parse(args[1]);
    existing = raw == null ? parsed : JSON.parse(raw);
  } catch (_error) {
    return JSON.stringify({ ok: false, error: "Invalid expense update." });
  }
  if (raw == null && (String(parsed.id || "") !== id || parsed.source !== "invoice")) {
    return JSON.stringify({ ok: false, error: "Expense not found." });
  }
  var entry = normalizeExpense(parsed);
  if (!entry) return JSON.stringify({ ok: false, error: "Enter a valid total." });
  entry.id = existing.id;
  entry.blob_name = existing.blob_name || "";
  entry.source = existing.source || "manual";
  entry.note = existing.note || "";
  entry.provider = existing.provider || "";
  entry.model = existing.model || "";
  entry.reviewed = true;
  entry.created_at = existing.created_at || new Date().toISOString();
  saveExpense(entry);
  return stateJsonWith(entry);
}

function normalizePocket(value) {
  if (!value || typeof value !== "object") return null;
  var name = safeText(value.name, "", 80);
  var target = money(value.target, null);
  var balance = money(value.balance, null);
  if (!name || target == null || target <= 0 || balance == null) return null;
  return {
    name: name,
    target: target,
    balance: balance,
    color: oneOf(
      value.color,
      ["mint", "blue", "violet", "amber", "coral"],
      "mint"
    )
  };
}

function createPocket(args, usage) {
  if (args.length < 1) return usage();
  var parsed;
  try {
    parsed = JSON.parse(args[0]);
  } catch (_error) {
    return JSON.stringify({ ok: false, error: "Invalid pocket details." });
  }
  var pocket = normalizePocket(parsed);
  if (!pocket) {
    return JSON.stringify({ ok: false, error: "Give the pocket a name, target, and valid balance." });
  }
  pocket.id = nextId(POCKET_SEQ_KEY);
  pocket.created_at = new Date().toISOString();
  kv.set(POCKET_PREFIX + pocket.id, JSON.stringify(pocket));
  return stateJson();
}

function updatePocket(args, usage) {
  if (args.length < 2) return usage();
  var id = String(args[0] || "");
  var raw = kvGet(POCKET_PREFIX + id);
  if (raw == null) return JSON.stringify({ ok: false, error: "Pocket not found." });
  var existing;
  var parsed;
  try {
    existing = JSON.parse(raw);
    parsed = JSON.parse(args[1]);
  } catch (_error) {
    return JSON.stringify({ ok: false, error: "Invalid pocket update." });
  }
  var pocket = normalizePocket(parsed);
  if (!pocket) {
    return JSON.stringify({ ok: false, error: "Give the pocket a name, target, and valid balance." });
  }
  pocket.id = id;
  pocket.created_at = existing.created_at || new Date().toISOString();
  kv.set(POCKET_PREFIX + id, JSON.stringify(pocket));
  return stateJson();
}

function configure(args) {
  var settings = saveSettings({
    provider: args.length > 0 ? args[0] : DEFAULT_PROVIDER,
    model: args.length > 1 ? args[1] : DEFAULT_OPENCODE_MODEL
  });
  return JSON.stringify({ ok: true, settings: settings });
}

var actions = {
  state: {
    summary: "Return office expenses, accounting totals, budget pockets, and vision settings.",
    args: [],
    returns: "JSON Spending state.",
    run: function () { return stateJson(); }
  },
  analyze_invoice: {
    summary: "Store an invoice image and extract a structured expense with vision AI.",
    args: [
      { name: "image_base64", required: true },
      { name: "mime", required: true },
      { name: "context", required: false },
      { name: "provider", required: false },
      { name: "model", required: false },
      { name: "mode", required: false }
    ],
    returns: "JSON Spending state and extracted expense.",
    run: analyzeInvoice
  },
  analyze_blob: {
    summary: "Analyze an invoice image previously imported with terrane blob put.",
    args: [
      { name: "blob_name", required: true },
      { name: "context", required: false },
      { name: "provider", required: false },
      { name: "model", required: false }
    ],
    returns: "JSON Spending state and extracted expense.",
    run: analyzeBlob
  },
  create_manual: {
    summary: "Create an office expense without an invoice image.",
    args: [{ name: "expense_json", required: true }],
    returns: "JSON Spending state and created expense.",
    run: createManual
  },
  update_expense: {
    summary: "Save a reviewed expense.",
    args: [
      { name: "id", required: true },
      { name: "expense_json", required: true }
    ],
    returns: "JSON Spending state and updated expense.",
    run: updateExpense
  },
  create_pocket: {
    summary: "Create a goal-based office budget pocket.",
    args: [{ name: "pocket_json", required: true }],
    returns: "JSON Spending state.",
    run: createPocket
  },
  update_pocket: {
    summary: "Update a budget pocket target or balance.",
    args: [
      { name: "id", required: true },
      { name: "pocket_json", required: true }
    ],
    returns: "JSON Spending state.",
    run: updatePocket
  },
  configure: {
    summary: "Save the default vision provider and model for Spending.",
    args: [
      { name: "provider", required: true },
      { name: "model", required: false }
    ],
    returns: "JSON settings.",
    run: configure
  },
  "common.list": {
    summary: "List saved expenses as addressable office-spending items.",
    args: [{ name: "filterJson", required: false }],
    returns: "A JSON array of expense item summaries.",
    run: function () {
      return JSON.stringify(expenses().map(function (expense) {
        return {
          id: expense.id,
          title: expense.vendor + " · " + expense.currency + " " + expense.total,
          kind: "expense",
          subtitle: [expense.category, expense.invoice_date].filter(Boolean).join(" · ")
        };
      }));
    }
  },
  "common.get": {
    summary: "Read one saved expense as an addressable item.",
    args: [{ name: "id", required: true }],
    returns: "Expense JSON or typed not-found JSON.",
    run: function (args) {
      var id = String(args[0] || "");
      var raw = kvGet(EXPENSE_PREFIX + id);
      if (raw == null) {
        return JSON.stringify({
          ok: false,
          error: { code: "NotFound", id: id }
        });
      }
      var expense = JSON.parse(raw);
      return JSON.stringify({
        id: expense.id,
        title: expense.vendor,
        kind: "expense",
        expense: expense
      });
    }
  },
  "common.receive": {
    summary: "Receive one JSON expense from another Terrane app.",
    args: [
      { name: "format", required: true },
      { name: "payload", required: true }
    ],
    returns: "Created expense state or a typed validation error.",
    run: function (args) {
      if (String(args[0] || "") !== "json") {
        return JSON.stringify({
          ok: false,
          error: { code: "UnsupportedFormat", format: String(args[0] || "") }
        });
      }
      return createManual([String(args[1] || "{}")], function () {
        return JSON.stringify({
          ok: false,
          error: { code: "InvalidInput", message: "Expected one JSON expense." }
        });
      });
    }
  }
};
