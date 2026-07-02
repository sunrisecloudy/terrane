// Chat backend for Terrane (UI + CLI): conversations with on-device models.
//
// The model context lives in the PLATFORM transcript — `lm.chat`/`lm.chatModel`
// feed back this app's recorded exchanges (per model), and `lm.resetChat`
// clears them — so replies genuinely follow the conversation without this app
// stuffing history into prompts. The kv keys below only mirror what the UI
// renders:
//
//   seq        -> highest message id ever allocated, as a decimal string
//   msg:<id>   -> one rendered message as JSON {role, text, model}
//   model      -> the selected model id ("" / missing = the home's default)
//
// Every mutation is exactly one recorded kv.* / local-model.* event, so
// Option-A replay rebuilds the chat by folding events, never by re-running
// this JS (and never by re-running inference).

var kv = ctx.resource.kv;
var lm = ctx.resource["local-model"];

var SEQ_KEY = "seq";
var MSG_PREFIX = "msg:";
var MODEL_KEY = "model";

var description = "Chat with on-device AI models: pick a registered model or pull one from Hugging Face, then talk.";

function readSeq() {
  var raw = kv.get(SEQ_KEY);
  var n = raw == null ? 0 : parseInt(raw, 10);
  return isNaN(n) || n < 0 ? 0 : n;
}

function pad(n) {
  var s = String(n);
  while (s.length < 8) s = "0" + s;
  return s;
}

function appendMessage(role, text, model) {
  var id = readSeq() + 1;
  kv.set(SEQ_KEY, String(id));
  kv.set(MSG_PREFIX + pad(id), JSON.stringify({ role: role, text: text, model: model }));
}

function readMessages() {
  var all = kv.all();
  var keys = [];
  for (var key in all) {
    if (!Object.prototype.hasOwnProperty.call(all, key)) continue;
    if (key.indexOf(MSG_PREFIX) === 0) keys.push(key);
  }
  keys.sort();
  var messages = [];
  for (var i = 0; i < keys.length; i++) {
    try {
      messages.push(JSON.parse(all[keys[i]]));
    } catch (e) {
      // A malformed record renders as raw text rather than hiding it.
      messages.push({ role: "assistant", text: String(all[keys[i]]), model: null });
    }
  }
  return messages;
}

function clearMessages() {
  var all = kv.all();
  for (var key in all) {
    if (!Object.prototype.hasOwnProperty.call(all, key)) continue;
    if (key.indexOf(MSG_PREFIX) === 0) kv.rm(key);
  }
  kv.set(SEQ_KEY, "0");
}

function selectedModel() {
  var model = kv.get(MODEL_KEY);
  return model == null || model === "" ? null : model;
}

function registeredModels() {
  var raw = lm.models();
  try {
    return JSON.parse(raw == null ? "[]" : raw);
  } catch (e) {
    return [];
  }
}

function stateJson() {
  return JSON.stringify({
    ok: true,
    models: registeredModels(),
    selected: selectedModel(),
    messages: readMessages()
  });
}

var actions = {
  send: {
    summary: "Send a chat message; the reply comes from the selected on-device model.",
    args: [{ name: "message", required: true }],
    run: function (args, usage) {
      if (args.length === 0) return usage();
      var text = args.join(" ");
      var model = selectedModel();
      var reply = model ? lm.chatModel(model, text) : lm.chat(text);
      if (reply == null) {
        return JSON.stringify({ ok: false, error: "generation failed; see the event log" });
      }
      appendMessage("user", text, model);
      appendMessage("assistant", reply, model);
      return JSON.stringify({ ok: true, reply: reply, model: model });
    }
  },
  state: {
    summary: "Everything the UI renders: models, selected model, messages.",
    args: [],
    run: function () { return stateJson(); }
  },
  models: {
    summary: "Registered on-device models as JSON (id, backend, default).",
    args: [],
    run: function () { return JSON.stringify({ ok: true, models: registeredModels(), selected: selectedModel() }); }
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
    summary: "Download a model from Hugging Face and register it. A .gguf file selects llama_cpp; no file snapshots the repo for mlx.",
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
    summary: "Start a fresh conversation (clears the model's context and the visible history).",
    args: [],
    run: function () {
      lm.resetChat();
      clearMessages();
      return JSON.stringify({ ok: true });
    }
  },
  history: {
    summary: "The visible conversation as JSON.",
    args: [],
    run: function () { return JSON.stringify({ ok: true, messages: readMessages() }); }
  }
};
