// Chat backend for Terrane (UI + CLI): multiple persisted conversations with
// on-device models. The original conversation remains on the legacy keys and
// local-model transcript, so existing homes retain both visible messages and
// continuation context. New conversations use local-model's app-scoped thread
// calls, keeping their recorded model contexts isolated without replaying
// inference or stuffing history into prompts.

var kv = ctx.resource.kv;
var lm = ctx.resource["local-model"];

var DEFAULT_CONVERSATION = "default";
var LEGACY_SEQ_KEY = "seq";
var LEGACY_MSG_PREFIX = "msg:";
var MODEL_KEY = "model";
var CONVERSATION_SEQ_KEY = "conversation:seq";
var SELECTED_CONVERSATION_KEY = "conversation:selected";
var CONVERSATION_PREFIX = "conversation:";
var META_SUFFIX = ":meta";

var description = "Chat with on-device AI models across multiple persisted conversations.";

function pad(n) {
  var s = String(n);
  while (s.length < 8) s = "0" + s;
  return s;
}

function conversationMetaKey(id) {
  return CONVERSATION_PREFIX + id + META_SUFFIX;
}

function messagePrefix(id) {
  return id === DEFAULT_CONVERSATION
    ? LEGACY_MSG_PREFIX
    : CONVERSATION_PREFIX + id + ":msg:";
}

function sequenceKey(id) {
  return id === DEFAULT_CONVERSATION
    ? LEGACY_SEQ_KEY
    : CONVERSATION_PREFIX + id + ":seq";
}

function readNumber(key) {
  var raw = kv.get(key);
  var n = raw == null ? 0 : parseInt(raw, 10);
  return isNaN(n) || n < 0 ? 0 : n;
}

function readMeta(id) {
  var raw = kv.get(conversationMetaKey(id));
  if (raw != null) {
    try {
      var meta = JSON.parse(raw);
      if (meta && typeof meta.title === "string" && meta.title) return meta;
    } catch (_) {}
  }
  return {
    title: id === DEFAULT_CONVERSATION ? "Conversation 1" : "New conversation"
  };
}

function writeMeta(id, meta) {
  kv.set(conversationMetaKey(id), JSON.stringify(meta));
}

function conversationIds() {
  var all = kv.all();
  var ids = [DEFAULT_CONVERSATION];
  for (var key in all) {
    if (!Object.prototype.hasOwnProperty.call(all, key)) continue;
    if (key.indexOf(CONVERSATION_PREFIX) !== 0 || key.slice(-META_SUFFIX.length) !== META_SUFFIX) {
      continue;
    }
    var id = key.slice(CONVERSATION_PREFIX.length, -META_SUFFIX.length);
    if (id && id !== DEFAULT_CONVERSATION && ids.indexOf(id) < 0) ids.push(id);
  }
  ids.sort(function (a, b) {
    if (a === DEFAULT_CONVERSATION) return -1;
    if (b === DEFAULT_CONVERSATION) return 1;
    return a < b ? -1 : a > b ? 1 : 0;
  });
  return ids;
}

function selectedConversation() {
  var selected = kv.get(SELECTED_CONVERSATION_KEY) || DEFAULT_CONVERSATION;
  return conversationIds().indexOf(selected) >= 0 ? selected : DEFAULT_CONVERSATION;
}

function appendMessage(conversation, role, text, model) {
  var seqKey = sequenceKey(conversation);
  var id = readNumber(seqKey) + 1;
  kv.set(seqKey, String(id));
  kv.set(
    messagePrefix(conversation) + pad(id),
    JSON.stringify({ role: role, text: text, model: model })
  );
}

function readMessages(conversation) {
  var all = kv.all();
  var prefix = messagePrefix(conversation);
  var keys = [];
  for (var key in all) {
    if (Object.prototype.hasOwnProperty.call(all, key) && key.indexOf(prefix) === 0) {
      keys.push(key);
    }
  }
  keys.sort();
  var messages = [];
  for (var i = 0; i < keys.length; i++) {
    try {
      messages.push(JSON.parse(all[keys[i]]));
    } catch (_) {
      messages.push({ role: "assistant", text: String(all[keys[i]]), model: null });
    }
  }
  return messages;
}

function selectedModel() {
  var model = kv.get(MODEL_KEY);
  return model == null || model === "" ? null : model;
}

function registeredModels() {
  var raw = lm.models();
  try {
    return JSON.parse(raw == null ? "[]" : raw);
  } catch (_) {
    return [];
  }
}

function conversationList() {
  var ids = conversationIds();
  var list = [];
  for (var i = 0; i < ids.length; i++) {
    var meta = readMeta(ids[i]);
    list.push({
      id: ids[i],
      title: meta.title,
      messageCount: readMessages(ids[i]).length
    });
  }
  return list;
}

function stateJson() {
  var selected = selectedConversation();
  return JSON.stringify({
    ok: true,
    models: registeredModels(),
    selected: selectedModel(),
    selectedConversation: selected,
    conversations: conversationList(),
    messages: readMessages(selected)
  });
}

function titleFromMessage(text) {
  var clean = String(text).replace(/\s+/g, " ").trim();
  if (clean.length > 42) clean = clean.slice(0, 41) + "…";
  return clean || "New conversation";
}

var actions = {
  send: {
    summary: "Send a message in the selected conversation using the selected on-device model.",
    args: [{ name: "message", required: true }],
    run: function (args, usage) {
      if (args.length === 0) return usage();
      var text = args.join(" ");
      var model = selectedModel();
      var conversation = selectedConversation();
      var reply;
      if (conversation === DEFAULT_CONVERSATION) {
        reply = model ? lm.chatModel(model, text) : lm.chat(text);
      } else {
        reply = model
          ? lm.chatThreadModel(conversation, model, text)
          : lm.chatThread(conversation, text);
      }
      if (reply == null) {
        return JSON.stringify({ ok: false, error: "generation failed; see the event log" });
      }
      if (readMessages(conversation).length === 0) {
        writeMeta(conversation, { title: titleFromMessage(text) });
      }
      appendMessage(conversation, "user", text, model);
      appendMessage(conversation, "assistant", reply, model);
      return JSON.stringify({
        ok: true,
        reply: reply,
        model: model,
        conversation: conversation
      });
    }
  },
  state: {
    summary: "Everything the UI renders: models, conversations, selection, and messages.",
    args: [],
    run: function () { return stateJson(); }
  },
  models: {
    summary: "Registered on-device models as JSON (id, backend, default).",
    args: [],
    run: function () {
      return JSON.stringify({
        ok: true,
        models: registeredModels(),
        selected: selectedModel()
      });
    }
  },
  use: {
    summary: "Chat with a specific registered model (empty id returns to the home default).",
    args: [{ name: "model-id", required: false }],
    run: function (args) {
      var id = args.length === 0 ? "" : args[0];
      if (id === "") {
        kv.set(MODEL_KEY, "");
        return JSON.stringify({ ok: true, selected: null });
      }
      var models = registeredModels();
      for (var i = 0; i < models.length; i++) {
        if (models[i].id === id) {
          kv.set(MODEL_KEY, id);
          return JSON.stringify({ ok: true, selected: id });
        }
      }
      return JSON.stringify({ ok: false, error: "unknown model: " + id + " (try models)" });
    }
  },
  pull: {
    summary: "Download a model from Hugging Face and register it.",
    args: [{ name: "org/repo", required: true }, { name: "file.gguf", required: false }],
    run: function (args, usage) {
      if (args.length === 0) return usage();
      var id = args.length > 1 ? lm.pullModel(args[0], args[1]) : lm.pullModel(args[0]);
      if (id == null) {
        return JSON.stringify({ ok: false, error: "pull failed; see the event log" });
      }
      kv.set(MODEL_KEY, id);
      return JSON.stringify({ ok: true, model: id, selected: id });
    }
  },
  "new": {
    summary: "Create and select a new persisted conversation.",
    args: [],
    run: function () {
      var next = readNumber(CONVERSATION_SEQ_KEY) + 1;
      var id = "c" + pad(next);
      kv.set(CONVERSATION_SEQ_KEY, String(next));
      writeMeta(id, { title: "New conversation" });
      kv.set(SELECTED_CONVERSATION_KEY, id);
      return stateJson();
    }
  },
  select: {
    summary: "Select a persisted conversation by id.",
    args: [{ name: "conversation-id", required: true }],
    run: function (args, usage) {
      if (args.length === 0) return usage();
      var id = args[0];
      if (conversationIds().indexOf(id) < 0) {
        return JSON.stringify({ ok: false, error: "unknown conversation: " + id });
      }
      kv.set(SELECTED_CONVERSATION_KEY, id);
      return stateJson();
    }
  },
  conversations: {
    summary: "Conversation identities, titles, counts, and current selection.",
    args: [],
    run: function () {
      return JSON.stringify({
        ok: true,
        conversations: conversationList(),
        selectedConversation: selectedConversation()
      });
    }
  },
  history: {
    summary: "The selected conversation as JSON.",
    args: [],
    run: function () {
      return JSON.stringify({
        ok: true,
        conversation: selectedConversation(),
        messages: readMessages(selectedConversation())
      });
    }
  }
};
