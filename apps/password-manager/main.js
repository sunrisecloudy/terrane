// Password Manager backend for Terrane (UI + CLI + MCP).
//
// SECURITY MODEL
// --------------
// Terrane's event log is plaintext, so this app never stores a secret through a
// bare kv.set. Every item is encrypted first via ctx.resource.crypto (Argon2id
// master-password key + XChaCha20-Poly1305). Only ciphertext and non-secret
// metadata reach kv, so replaying the log rebuilds the vault without ever
// exposing plaintext or the master password. crypto's methods are reads, so they
// record nothing themselves.
//
// kv keyspace (all values are strings):
//   meta          -> vault meta JSON (salt, KDF params, verifier) — NOT secret
//   seq/fseq/aseq -> id counters (decimal) for items/folders/audit
//   item:<id>     -> sealed ciphertext blob of the full item JSON
//   folder:<id>   -> sealed ciphertext blob of { name }
//   trash:<id>    -> sealed ciphertext blob of { item, deletedAt }
//   audit:<id>    -> plaintext { ts, action, item, detail } — reviewable trail
//   settings      -> plaintext settings JSON
//
// AUTH
// ----
// Secret operations take an `auth` argument that is EITHER an unlocked session id
// (from `unlock`, ideal for the UI / a long-lived MCP session) OR the master
// password itself (each CLI command is its own process, so it unlocks inline).
// resolveAuth() figures out which and, for the master-password path, locks the
// throwaway session again when the call finishes.

var kv = ctx.resource.kv;
var crypto = ctx.resource.crypto;

var META_KEY = "meta";
var ITEM_PREFIX = "item:";
var FOLDER_PREFIX = "folder:";
var TRASH_PREFIX = "trash:";
var AUDIT_PREFIX = "audit:";
var SETTINGS_KEY = "settings";

// ---- small helpers ---------------------------------------------------------

function now() {
  try {
    return Date.now();
  } catch (e) {
    return 0;
  }
}

function ok(obj) {
  obj = obj || {};
  obj.ok = true;
  return JSON.stringify(obj);
}

function err(reason) {
  return JSON.stringify({ ok: false, error: reason });
}

// Call a crypto read method and parse its { ok, ... } envelope.
function cr(method) {
  var args = Array.prototype.slice.call(arguments, 1);
  var raw = crypto[method].apply(crypto, args);
  if (raw == null) return { ok: false, reason: "crypto_unavailable" };
  try {
    return JSON.parse(raw);
  } catch (e) {
    return { ok: false, reason: "crypto_parse" };
  }
}

function readCounter(key) {
  var raw = kv.get(key);
  if (raw == null) return 0;
  var n = parseInt(raw, 10);
  return isNaN(n) || n < 0 ? 0 : n;
}

function nextId(key) {
  var n = readCounter(key) + 1;
  kv.set(key, String(n));
  return n;
}

function hasVault() {
  return kv.get(META_KEY) != null;
}

// Numeric ids of every stored key under a prefix.
function idsWithPrefix(prefix) {
  var all = kv.all();
  var ids = [];
  for (var key in all) {
    if (!Object.prototype.hasOwnProperty.call(all, key)) continue;
    if (key.indexOf(prefix) !== 0) continue;
    var id = parseInt(key.slice(prefix.length), 10);
    if (!isNaN(id)) ids.push(id);
  }
  ids.sort(function (a, b) {
    return a - b;
  });
  return ids;
}

function audit(action, item, detail) {
  var id = nextId("aseq");
  kv.set(
    AUDIT_PREFIX + id,
    JSON.stringify({
      ts: now(),
      action: action,
      item: item == null ? null : item,
      detail: detail == null ? null : detail,
    })
  );
}

// ---- auth resolution -------------------------------------------------------

// Returns { session, temp } or { error }. `temp` sessions are locked by the
// caller (via withSession) once the operation completes.
function resolveAuth(auth) {
  if (auth == null || auth === "") return { error: "no_auth" };
  var st = cr("status", auth);
  if (st.ok && st.unlocked) return { session: auth, temp: false };
  var meta = kv.get(META_KEY);
  if (meta == null) return { error: "no_vault" };
  var u = cr("unlock", auth, meta);
  if (!u.ok) return { error: u.reason || "bad_password" };
  return { session: u.session, temp: true };
}

// Resolve auth, run fn(session), then lock a throwaway session. fn returns the
// action's response string.
function withSession(auth, fn) {
  var r = resolveAuth(auth);
  if (r.error) return err(r.error);
  try {
    return fn(r.session);
  } finally {
    if (r.temp) cr("lock", r.session);
  }
}

// ---- item store ------------------------------------------------------------

function sealItem(session, obj) {
  var s = cr("seal", session, JSON.stringify(obj));
  if (!s.ok) return null;
  return s.blob;
}

function openBlob(session, blob) {
  if (blob == null) return null;
  var o = cr("open", session, blob);
  if (!o.ok) return null;
  try {
    return JSON.parse(o.plaintext);
  } catch (e) {
    return null;
  }
}

function loadItem(session, id) {
  return openBlob(session, kv.get(ITEM_PREFIX + id));
}

function saveItem(session, item) {
  var blob = sealItem(session, item);
  if (blob == null) return false;
  kv.set(ITEM_PREFIX + item.id, blob);
  return true;
}

// Non-secret-ish metadata view of an item for lists/search. Note: names and
// usernames ARE decrypted from the sealed blob — listing requires unlock.
function itemMeta(item) {
  return {
    id: item.id,
    type: item.type || "login",
    name: item.name || "",
    username: item.username || "",
    uris: item.uris || [],
    folder: item.folder == null ? null : item.folder,
    favorite: !!item.favorite,
    hasTotp: !!item.totp,
    updated: item.updated || 0,
  };
}

// Resolve an item reference that may be a numeric id or a (case-insensitive)
// name. Returns the item object or null.
function findItem(session, ref) {
  var byId = parseInt(ref, 10);
  var ids = idsWithPrefix(ITEM_PREFIX);
  var i;
  if (!isNaN(byId)) {
    for (i = 0; i < ids.length; i++) {
      if (ids[i] === byId) return loadItem(session, byId);
    }
  }
  var needle = String(ref).toLowerCase();
  for (i = 0; i < ids.length; i++) {
    var it = loadItem(session, ids[i]);
    if (it && String(it.name || "").toLowerCase() === needle) return it;
  }
  return null;
}

// Extract a base32 TOTP secret + options from a stored value that may be a raw
// base32 secret or an otpauth:// URI.
function totpParams(value) {
  if (value == null || value === "") return null;
  var v = String(value);
  if (v.indexOf("otpauth://") !== 0) {
    return { secret: v.replace(/\s+/g, "") };
  }
  var params = { secret: "" };
  var q = v.indexOf("?");
  if (q < 0) return null;
  var pairs = v.slice(q + 1).split("&");
  for (var i = 0; i < pairs.length; i++) {
    var kvp = pairs[i].split("=");
    var key = decodeURIComponent(kvp[0] || "");
    var val = decodeURIComponent(kvp[1] || "");
    if (key === "secret") params.secret = val;
    else if (key === "digits") params.digits = parseInt(val, 10);
    else if (key === "period") params.period = parseInt(val, 10);
    else if (key === "algorithm") params.algorithm = val;
  }
  return params.secret ? params : null;
}

// ---- actions ---------------------------------------------------------------

var description =
  "A secure, Bitwarden-style password manager. Items are encrypted with a " +
  "master-password-derived key; only ciphertext is stored. Works from the UI, " +
  "the CLI, and MCP. Secret operations take `auth` = an unlocked session id " +
  "(from `unlock`) or the master password itself.";

var AUTH_ARG = {
  name: "auth",
  required: true,
  summary: "an unlocked session id (from `unlock`) or the master password",
};

var actions = {
  status: {
    summary: "Whether a vault exists and whether the given session is unlocked.",
    args: [{ name: "session", required: false, summary: "a session id to check" }],
    returns: '{ ok, exists, unlocked }',
    run: function (args) {
      var unlocked = false;
      if (args[0]) {
        var st = cr("status", args[0]);
        unlocked = !!(st.ok && st.unlocked);
      }
      return ok({ exists: hasVault(), unlocked: unlocked });
    },
  },

  init: {
    summary: "Create a new vault protected by a master password.",
    args: [{ name: "master", required: true, summary: "the master password" }],
    returns: '{ ok, session } or { ok:false, error:"vault_exists" }',
    run: function (args, usage) {
      var master = args[0];
      if (!master) return usage();
      if (hasVault()) return err("vault_exists");
      var v = cr("newVault", master);
      if (!v.ok) return err(v.reason || "init_failed");
      kv.set(META_KEY, v.meta);
      audit("vault.init", null, null);
      return ok({ session: v.session });
    },
  },

  unlock: {
    summary: "Unlock the vault and return a session id for later commands.",
    args: [{ name: "master", required: true, summary: "the master password" }],
    returns: '{ ok, session } or { ok:false, error:"bad_password" }',
    run: function (args, usage) {
      var master = args[0];
      if (!master) return usage();
      var meta = kv.get(META_KEY);
      if (meta == null) return err("no_vault");
      var u = cr("unlock", master, meta);
      if (!u.ok) return err(u.reason || "bad_password");
      return ok({ session: u.session });
    },
  },

  lock: {
    summary: "Lock a session, wiping its key from memory.",
    args: [{ name: "session", required: true, summary: "the session id to lock" }],
    returns: "{ ok }",
    run: function (args, usage) {
      if (!args[0]) return usage();
      cr("lock", args[0]);
      return ok({});
    },
  },

  add: {
    summary: "Add an item from a JSON object (any type + custom fields).",
    args: [
      AUTH_ARG,
      { name: "json", required: true, summary: 'item JSON, e.g. {"name":"GitHub","username":"me","password":"..."}' },
    ],
    returns: "{ ok, id, name }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var auth = args[0];
      var raw = args.slice(1).join(" ");
      var input;
      try {
        input = JSON.parse(raw);
      } catch (e) {
        return err("bad_json");
      }
      return withSession(auth, function (session) {
        var id = nextId("seq");
        var item = input || {};
        item.id = id;
        item.type = item.type || "login";
        item.name = item.name || "(unnamed)";
        item.created = now();
        item.updated = now();
        item.history = item.history || [];
        if (!saveItem(session, item)) return err("seal_failed");
        audit("item.add", id, item.name);
        return ok({ id: id, name: item.name });
      });
    },
  },

  "add-login": {
    summary: "Add a login item (convenience wrapper over `add`).",
    args: [
      AUTH_ARG,
      { name: "name", required: true, summary: "display name" },
      { name: "username", required: true, summary: "username or email" },
      { name: "password", required: true, summary: "the password" },
      { name: "uri", required: false, summary: "site URL for matching" },
    ],
    returns: "{ ok, id, name }",
    run: function (args, usage) {
      if (args.length < 4) return usage();
      var auth = args[0];
      var item = {
        type: "login",
        name: args[1],
        username: args[2],
        password: args[3],
        uris: args[4] ? [args[4]] : [],
      };
      return withSession(auth, function (session) {
        var id = nextId("seq");
        item.id = id;
        item.created = now();
        item.updated = now();
        item.history = [];
        if (!saveItem(session, item)) return err("seal_failed");
        audit("item.add", id, item.name);
        return ok({ id: id, name: item.name });
      });
    },
  },

  get: {
    summary: "Reveal a full item by id or name (audited).",
    args: [AUTH_ARG, { name: "ref", required: true, summary: "item id or name" }],
    returns: "{ ok, item }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      return withSession(args[0], function (session) {
        var item = findItem(session, args[1]);
        if (!item) return err("not_found");
        audit("item.reveal", item.id, item.name);
        return ok({ item: item });
      });
    },
  },

  password: {
    summary: "Reveal only the password of an item (audited).",
    args: [AUTH_ARG, { name: "ref", required: true, summary: "item id or name" }],
    returns: "{ ok, password }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      return withSession(args[0], function (session) {
        var item = findItem(session, args[1]);
        if (!item) return err("not_found");
        audit("item.reveal-password", item.id, item.name);
        return ok({ password: item.password || "" });
      });
    },
  },

  list: {
    summary: "List item metadata (no secrets). Optionally filter by folder id.",
    args: [AUTH_ARG, { name: "folder", required: false, summary: "folder id to filter by" }],
    returns: "{ ok, items: [ { id, name, type, username, folder, favorite, hasTotp } ] }",
    run: function (args, usage) {
      if (args.length < 1) return usage();
      var folder = args[1] != null && args[1] !== "" ? parseInt(args[1], 10) : null;
      return withSession(args[0], function (session) {
        var ids = idsWithPrefix(ITEM_PREFIX);
        var out = [];
        for (var i = 0; i < ids.length; i++) {
          var it = loadItem(session, ids[i]);
          if (!it) continue;
          if (folder != null && it.folder !== folder) continue;
          out.push(itemMeta(it));
        }
        return ok({ items: out });
      });
    },
  },

  search: {
    summary: "Search items by name, username, or URI (no secrets in results).",
    args: [AUTH_ARG, { name: "query", required: true, summary: "text to match" }],
    returns: "{ ok, items: [ metadata ] }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var q = args.slice(1).join(" ").toLowerCase();
      return withSession(args[0], function (session) {
        var ids = idsWithPrefix(ITEM_PREFIX);
        var out = [];
        for (var i = 0; i < ids.length; i++) {
          var it = loadItem(session, ids[i]);
          if (!it) continue;
          var hay = (
            (it.name || "") + " " + (it.username || "") + " " + (it.uris || []).join(" ")
          ).toLowerCase();
          if (hay.indexOf(q) >= 0) out.push(itemMeta(it));
        }
        return ok({ items: out });
      });
    },
  },

  edit: {
    summary: "Merge a JSON patch into an item (password change is versioned).",
    args: [
      AUTH_ARG,
      { name: "id", required: true, summary: "item id" },
      { name: "json", required: true, summary: "fields to change, e.g. {\"password\":\"new\"}" },
    ],
    returns: "{ ok, id }",
    run: function (args, usage) {
      if (args.length < 3) return usage();
      var id = parseInt(args[1], 10);
      if (isNaN(id)) return usage();
      var patch;
      try {
        patch = JSON.parse(args.slice(2).join(" "));
      } catch (e) {
        return err("bad_json");
      }
      return withSession(args[0], function (session) {
        var item = loadItem(session, id);
        if (!item) return err("not_found");
        if (
          patch.password != null &&
          patch.password !== item.password &&
          item.password != null
        ) {
          item.history = item.history || [];
          item.history.push({ password: item.password, changedAt: item.updated || now() });
        }
        for (var k in patch) {
          if (Object.prototype.hasOwnProperty.call(patch, k)) item[k] = patch[k];
        }
        item.id = id;
        item.updated = now();
        if (!saveItem(session, item)) return err("seal_failed");
        audit("item.edit", id, item.name);
        return ok({ id: id });
      });
    },
  },

  rm: {
    summary: "Move an item to the trash.",
    args: [AUTH_ARG, { name: "id", required: true, summary: "item id" }],
    returns: "{ ok }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var id = parseInt(args[1], 10);
      if (isNaN(id)) return usage();
      return withSession(args[0], function (session) {
        var item = loadItem(session, id);
        if (!item) return err("not_found");
        var blob = sealItem(session, { item: item, deletedAt: now() });
        if (blob == null) return err("seal_failed");
        kv.set(TRASH_PREFIX + id, blob);
        kv.rm(ITEM_PREFIX + id);
        audit("item.delete", id, item.name);
        return ok({});
      });
    },
  },

  "trash-list": {
    summary: "List trashed items (metadata).",
    args: [AUTH_ARG],
    returns: "{ ok, items }",
    run: function (args, usage) {
      if (args.length < 1) return usage();
      return withSession(args[0], function (session) {
        var ids = idsWithPrefix(TRASH_PREFIX);
        var out = [];
        for (var i = 0; i < ids.length; i++) {
          var rec = openBlob(session, kv.get(TRASH_PREFIX + ids[i]));
          if (rec && rec.item) {
            var m = itemMeta(rec.item);
            m.deletedAt = rec.deletedAt || 0;
            out.push(m);
          }
        }
        return ok({ items: out });
      });
    },
  },

  restore: {
    summary: "Restore a trashed item.",
    args: [AUTH_ARG, { name: "id", required: true, summary: "trashed item id" }],
    returns: "{ ok, id }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var id = parseInt(args[1], 10);
      if (isNaN(id)) return usage();
      return withSession(args[0], function (session) {
        var rec = openBlob(session, kv.get(TRASH_PREFIX + id));
        if (!rec || !rec.item) return err("not_found");
        if (!saveItem(session, rec.item)) return err("seal_failed");
        kv.rm(TRASH_PREFIX + id);
        audit("item.restore", id, rec.item.name);
        return ok({ id: id });
      });
    },
  },

  purge: {
    summary: "Permanently delete a trashed item.",
    args: [AUTH_ARG, { name: "id", required: true, summary: "trashed item id" }],
    returns: "{ ok }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var id = parseInt(args[1], 10);
      if (isNaN(id)) return usage();
      if (kv.get(TRASH_PREFIX + id) == null) return err("not_found");
      kv.rm(TRASH_PREFIX + id);
      audit("item.purge", id, null);
      return ok({});
    },
  },

  "folder-add": {
    summary: "Create a folder.",
    args: [AUTH_ARG, { name: "name", required: true, summary: "folder name" }],
    returns: "{ ok, id, name }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var name = args.slice(1).join(" ");
      return withSession(args[0], function (session) {
        var id = nextId("fseq");
        var blob = sealItem(session, { id: id, name: name });
        if (blob == null) return err("seal_failed");
        kv.set(FOLDER_PREFIX + id, blob);
        audit("folder.add", id, name);
        return ok({ id: id, name: name });
      });
    },
  },

  "folder-list": {
    summary: "List folders.",
    args: [AUTH_ARG],
    returns: "{ ok, folders: [ { id, name } ] }",
    run: function (args, usage) {
      if (args.length < 1) return usage();
      return withSession(args[0], function (session) {
        var ids = idsWithPrefix(FOLDER_PREFIX);
        var out = [];
        for (var i = 0; i < ids.length; i++) {
          var f = openBlob(session, kv.get(FOLDER_PREFIX + ids[i]));
          if (f) out.push({ id: ids[i], name: f.name || "" });
        }
        return ok({ folders: out });
      });
    },
  },

  "folder-rm": {
    summary: "Delete a folder (items keep their folder id but become unfiled).",
    args: [AUTH_ARG, { name: "id", required: true, summary: "folder id" }],
    returns: "{ ok }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var id = parseInt(args[1], 10);
      if (isNaN(id)) return usage();
      if (kv.get(FOLDER_PREFIX + id) == null) return err("not_found");
      kv.rm(FOLDER_PREFIX + id);
      audit("folder.rm", id, null);
      return ok({});
    },
  },

  generate: {
    summary: "Generate a random password.",
    args: [{ name: "options", required: false, summary: 'JSON: {length,uppercase,lowercase,digits,symbols,avoid_ambiguous}' }],
    returns: "{ ok, password }",
    run: function (args) {
      var opts = args.length ? args.join(" ") : "{}";
      var r = cr("generatePassword", opts);
      if (!r.ok) return err(r.reason || "generate_failed");
      return ok({ password: r.password });
    },
  },

  passphrase: {
    summary: "Generate a diceware passphrase.",
    args: [{ name: "options", required: false, summary: 'JSON: {words,separator,capitalize,include_number}' }],
    returns: "{ ok, passphrase }",
    run: function (args) {
      var opts = args.length ? args.join(" ") : "{}";
      var r = cr("generatePassphrase", opts);
      if (!r.ok) return err(r.reason || "generate_failed");
      return ok({ passphrase: r.passphrase });
    },
  },

  strength: {
    summary: "Score a password's strength (0-4).",
    args: [{ name: "password", required: true, summary: "the password to score" }],
    returns: "{ ok, score, guessesLog10 }",
    run: function (args, usage) {
      if (!args.length) return usage();
      var r = cr("strength", args.join(" "));
      if (!r.ok) return err("strength_failed");
      return ok({ score: r.score, guessesLog10: r.guessesLog10 });
    },
  },

  totp: {
    summary: "Current TOTP 2FA code for an item's stored secret (audited).",
    args: [AUTH_ARG, { name: "ref", required: true, summary: "item id or name" }],
    returns: "{ ok, code, remaining, period }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      return withSession(args[0], function (session) {
        var item = findItem(session, args[1]);
        if (!item) return err("not_found");
        var params = totpParams(item.totp);
        if (!params) return err("no_totp");
        var r = cr("totp", JSON.stringify(params));
        if (!r.ok) return err(r.reason || "totp_failed");
        audit("item.totp", item.id, item.name);
        return ok({ code: r.code, remaining: r.remaining, period: r.period });
      });
    },
  },

  health: {
    summary: "Vault health report: weak, reused, old, and no-2FA items.",
    args: [AUTH_ARG],
    returns: "{ ok, total, weak, reused, old, missing2fa }",
    run: function (args, usage) {
      if (args.length < 1) return usage();
      var OLD_MS = 180 * 24 * 60 * 60 * 1000;
      return withSession(args[0], function (session) {
        var ids = idsWithPrefix(ITEM_PREFIX);
        var items = [];
        var i;
        for (i = 0; i < ids.length; i++) {
          var it = loadItem(session, ids[i]);
          if (it) items.push(it);
        }
        var counts = {};
        for (i = 0; i < items.length; i++) {
          var p = items[i].password;
          if (p) counts[p] = (counts[p] || 0) + 1;
        }
        var weak = [], reused = [], old = [], missing2fa = [];
        var current = now();
        for (i = 0; i < items.length; i++) {
          var item = items[i];
          if (item.password) {
            var s = cr("strength", item.password);
            if (s.ok && s.score < 2) weak.push(item.id);
            if (counts[item.password] > 1) reused.push(item.id);
          }
          if (item.updated && current - item.updated > OLD_MS) old.push(item.id);
          if ((item.type || "login") === "login" && item.password && !item.totp) {
            missing2fa.push(item.id);
          }
        }
        return ok({
          total: items.length,
          weak: weak,
          reused: reused,
          old: old,
          missing2fa: missing2fa,
        });
      });
    },
  },

  export: {
    summary: "Export every item as decrypted JSON (audited; handle with care).",
    args: [AUTH_ARG],
    returns: "{ ok, warning, items: [ full items ] }",
    run: function (args, usage) {
      if (args.length < 1) return usage();
      return withSession(args[0], function (session) {
        var ids = idsWithPrefix(ITEM_PREFIX);
        var out = [];
        for (var i = 0; i < ids.length; i++) {
          var it = loadItem(session, ids[i]);
          if (it) out.push(it);
        }
        audit("vault.export", null, out.length);
        return ok({
          warning: "This payload contains decrypted secrets in plaintext.",
          items: out,
        });
      });
    },
  },

  "import": {
    summary: "Import an array of items (generic or Bitwarden-style JSON).",
    args: [AUTH_ARG, { name: "json", required: true, summary: "JSON array of items" }],
    returns: "{ ok, imported }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var parsed;
      try {
        parsed = JSON.parse(args.slice(1).join(" "));
      } catch (e) {
        return err("bad_json");
      }
      var rows = parsed && parsed.items ? parsed.items : parsed;
      if (!rows || typeof rows.length !== "number") return err("bad_json");
      return withSession(args[0], function (session) {
        var count = 0;
        for (var i = 0; i < rows.length; i++) {
          var item = normalizeImport(rows[i]);
          if (!item) continue;
          item.id = nextId("seq");
          item.created = now();
          item.updated = now();
          item.history = [];
          if (saveItem(session, item)) count++;
        }
        audit("vault.import", null, count);
        return ok({ imported: count });
      });
    },
  },

  "change-master": {
    summary: "Change the master password (re-encrypts every item under the new key).",
    args: [AUTH_ARG, { name: "newMaster", required: true, summary: "the new master password" }],
    returns: "{ ok, reencrypted }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var newMaster = args[1];
      return withSession(args[0], function (oldSession) {
        var v = cr("newVault", newMaster);
        if (!v.ok) return err(v.reason || "rekey_failed");
        var newSession = v.session;
        try {
          var moved = reseal(oldSession, newSession, ITEM_PREFIX) +
            reseal(oldSession, newSession, FOLDER_PREFIX) +
            reseal(oldSession, newSession, TRASH_PREFIX);
          kv.set(META_KEY, v.meta);
          audit("vault.change-master", null, moved);
          return ok({ reencrypted: moved });
        } finally {
          cr("lock", newSession);
        }
      });
    },
  },

  audit: {
    summary: "Recent audit trail (non-secret metadata; no unlock required).",
    args: [{ name: "limit", required: false, summary: "max entries (default 50)" }],
    returns: "{ ok, entries: [ { id, ts, action, item, detail } ] }",
    run: function (args) {
      var limit = args[0] ? parseInt(args[0], 10) : 50;
      if (isNaN(limit) || limit <= 0) limit = 50;
      var ids = idsWithPrefix(AUDIT_PREFIX);
      ids.reverse();
      var out = [];
      for (var i = 0; i < ids.length && out.length < limit; i++) {
        var raw = kv.get(AUDIT_PREFIX + ids[i]);
        if (raw == null) continue;
        try {
          var e = JSON.parse(raw);
          e.id = ids[i];
          out.push(e);
        } catch (err2) {
          // skip malformed
        }
      }
      return ok({ entries: out });
    },
  },

  settings: {
    summary: "Read app settings (non-secret).",
    args: [],
    returns: "{ ok, settings }",
    run: function () {
      var raw = kv.get(SETTINGS_KEY);
      var s = {};
      if (raw != null) {
        try {
          s = JSON.parse(raw);
        } catch (e) {
          s = {};
        }
      }
      return ok({ settings: s });
    },
  },

  "set-setting": {
    summary: "Set a single setting key to a value.",
    args: [
      { name: "key", required: true, summary: "setting key" },
      { name: "value", required: true, summary: "setting value" },
    ],
    returns: "{ ok }",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var raw = kv.get(SETTINGS_KEY);
      var s = {};
      if (raw != null) {
        try {
          s = JSON.parse(raw);
        } catch (e) {
          s = {};
        }
      }
      s[args[0]] = args.slice(1).join(" ");
      kv.set(SETTINGS_KEY, JSON.stringify(s));
      return ok({});
    },
  },
};

// Re-seal every value under a prefix from one session's key to another. Returns
// how many blobs were moved.
function reseal(oldSession, newSession, prefix) {
  var ids = idsWithPrefix(prefix);
  var moved = 0;
  for (var i = 0; i < ids.length; i++) {
    var obj = openBlob(oldSession, kv.get(prefix + ids[i]));
    if (obj == null) continue;
    var blob = sealItem(newSession, obj);
    if (blob == null) continue;
    kv.set(prefix + ids[i], blob);
    moved++;
  }
  return moved;
}

// Normalize an imported row (generic {name,username,password,...} or a
// Bitwarden export item with a nested `login`) into our item shape.
function normalizeImport(row) {
  if (!row || typeof row !== "object") return null;
  var item = { type: row.type || "login", name: row.name || "(imported)" };
  var login = row.login || row;
  if (login.username != null) item.username = login.username;
  if (login.password != null) item.password = login.password;
  if (login.totp != null) item.totp = login.totp;
  if (row.notes != null) item.notes = row.notes;
  var uris = login.uris || row.uris;
  if (uris) {
    item.uris = [];
    for (var i = 0; i < uris.length; i++) {
      var u = uris[i];
      item.uris.push(typeof u === "string" ? u : u && u.uri ? u.uri : "");
    }
  }
  return item;
}
